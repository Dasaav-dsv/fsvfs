use std::{
    ffi::OsStr,
    fs, io,
    ops::Deref,
    sync::Arc,
    time::{Duration, SystemTime},
};

use color_eyre::eyre;
use fuser::{
    AccessFlags, BackgroundSession, Config, Errno, FileAttr, FileHandle, FileType, FopenFlags,
    Generation, INodeNo, KernelConfig, LockOwner, MountOption, OpenAccMode, OpenFlags, ReplyAttr,
    ReplyData, ReplyDirectory, ReplyDirectoryPlus, ReplyEmpty, ReplyEntry, ReplyOpen, ReplyStatfs,
    ReplyXattr, Request, spawn_mount2,
};
use tracing::warn;

use crate::{
    dvdbnd::{
        filesystem::{DvdbndFile, DvdbndFilesystem},
        mount::DvdbndMount,
    },
    filesystem::{Entry, EntryKind, ReadOnlyFilesystem},
    thread::{OnInterrupt, run_until_interrupted},
};

const MAX_NAME: u32 = 255;
const BLOCK_SIZE: u32 = 512;

const CACHE_TTL: Duration = Duration::new(60, 0);

#[repr(transparent)]
struct ArcDvdbndMount<F: DvdbndFilesystem>(pub Arc<DvdbndMount<F>>);

impl<F> DvdbndMount<F>
where
    F: DvdbndFilesystem + Send + Sync + 'static,
{
    pub fn mount(self, mountpoint: &str) -> eyre::Result<()> {
        fs::create_dir_all(mountpoint)?;

        let mut config = Config::default();
        config.mount_options = vec![
            MountOption::FSName("fsvfs".to_string()),
            MountOption::RO,
            MountOption::Async,
        ];

        #[cfg(target_os = "linux")]
        {
            config.clone_fd = true;
            config.n_threads = Some(4);
        }

        let mount = spawn_mount2(ArcDvdbndMount::new(self), mountpoint, &config)?;

        run_until_interrupted(mount)?;

        Ok(())
    }

    fn file_attr(&self, ino: INodeNo, entry: &Entry<'_, DvdbndFile>) -> FileAttr {
        let kind = file_type(entry);

        let (size, blocks) = if let Entry {
            kind: EntryKind::File(data),
            ..
        } = entry
        {
            let size = data.unpadded_len();
            (size as u64, size.div_ceil(BLOCK_SIZE) as u64)
        } else {
            (0, 0)
        };

        FileAttr {
            ino,
            size,
            blocks,
            atime: self.timestamp,
            mtime: self.timestamp,
            ctime: self.timestamp,
            crtime: self.timestamp,
            kind,
            perm: 0o555,
            nlink: 1,
            uid: 0,
            gid: 0,
            rdev: 0,
            blksize: BLOCK_SIZE,
            flags: 0,
        }
    }
}

impl<F: DvdbndFilesystem> ArcDvdbndMount<F> {
    fn new(mount: DvdbndMount<F>) -> Self {
        Self(Arc::new(mount))
    }
}

impl<F> fuser::Filesystem for ArcDvdbndMount<F>
where
    F: DvdbndFilesystem + Send + Sync + 'static,
{
    fn init(&mut self, _req: &Request, config: &mut KernelConfig) -> io::Result<()> {
        Arc::get_mut(&mut self.0).unwrap().timestamp = SystemTime::now();

        let _ = config.set_max_stack_depth(1);

        Ok(())
    }

    fn destroy(&mut self) {}

    fn lookup(&self, _req: &Request, parent: INodeNo, name: &OsStr, reply: ReplyEntry) {
        let fs = self.fs.as_rofs();

        let Some(name) = name.to_str() else {
            reply.error(Errno::ENOSYS);
            return;
        };

        let Ok(ino) = fs.lookup_in_dir(parent.0, name) else {
            reply.error(Errno::ENOENT);
            return;
        };

        let Ok(entry) = self.fs.as_rofs().entry(ino) else {
            reply.error(Errno::ENOENT);
            return;
        };

        let attr = self.file_attr(INodeNo(ino), &entry);

        reply.entry(&CACHE_TTL, &attr, Generation(0));
    }

    fn getattr(&self, _req: &Request, ino: INodeNo, _fh: Option<FileHandle>, reply: ReplyAttr) {
        let Ok(entry) = self.fs.as_rofs().entry(ino.0) else {
            reply.error(Errno::ENOENT);
            return;
        };

        let attr = self.file_attr(ino, &entry);

        reply.attr(&CACHE_TTL, &attr);
    }

    fn open(&self, _req: &Request, ino: INodeNo, flags: OpenFlags, reply: ReplyOpen) {
        if flags.acc_mode() == OpenAccMode::O_RDONLY {
            reply.opened(
                FileHandle(ino.0),
                FopenFlags::FOPEN_NOFLUSH | FopenFlags::FOPEN_KEEP_CACHE,
            );
        } else {
            reply.error(Errno::EROFS);
        }
    }

    fn opendir(&self, _req: &Request, ino: INodeNo, flags: OpenFlags, reply: ReplyOpen) {
        if flags.acc_mode() != OpenAccMode::O_RDONLY {
            reply.error(Errno::EROFS);
            return;
        }

        match self.fs.as_rofs().entry(ino.0) {
            Ok(Entry {
                kind: EntryKind::Dir(_),
                ..
            }) => reply.opened(
                FileHandle(ino.0),
                FopenFlags::FOPEN_NOFLUSH | FopenFlags::FOPEN_CACHE_DIR,
            ),
            Ok(_) => reply.error(Errno::ENOTDIR),
            _ => reply.error(Errno::ENOENT),
        }
    }

    fn read(
        &self,
        _req: &Request,
        ino: INodeNo,
        fh: FileHandle,
        offset: u64,
        size: u32,
        _flags: OpenFlags,
        _lock_owner: Option<LockOwner>,
        reply: ReplyData,
    ) {
        if ino.0 == fh.0
            && let Ok(Entry {
                kind: EntryKind::File(file),
                ..
            }) = self.fs.as_rofs().entry(ino.0)
        {
            let file_len = file.unpadded_len();

            let offset = offset.min(file_len as u64) as u32;
            let len = (file_len - offset).min(size);

            let mount = self.0.clone();

            let reply = Arc::new(reply);
            let reply_if_err = Arc::downgrade(&reply);

            let res = self.dispatcher.dispatch(async move || {
                let mut data = Vec::with_capacity(len as usize);

                let res = mount
                    .read_file(ino.0, offset.into(), len, |buf, _| {
                        data.extend_from_slice(buf);
                        Ok(())
                    })
                    .await;

                let reply = Arc::into_inner(reply).expect("reply is uniquely owned");

                match res {
                    Ok(_) => reply.data(&data),
                    Err(e) => {
                        warn!("`DvdbndMount::read_file` error: {e}");
                        reply.error(Errno::EIO);
                    }
                }
            });

            if let Err(not_dispatched) = res {
                let reply = reply_if_err.upgrade();
                drop(not_dispatched);

                let reply = reply.and_then(Arc::into_inner).expect("reply was lost?");
                reply.error(Errno::EIO);
            }
        } else {
            reply.error(Errno::ESTALE);
        }
    }

    fn release(
        &self,
        _req: &Request,
        ino: INodeNo,
        fh: FileHandle,
        _flags: OpenFlags,
        _lock_owner: Option<LockOwner>,
        _flush: bool,
        reply: ReplyEmpty,
    ) {
        if ino.0 == fh.0
            && let Ok(Entry {
                kind: EntryKind::File(_),
                ..
            }) = self.fs.as_rofs().entry(ino.0)
        {
            reply.ok();
        } else {
            reply.error(Errno::ESTALE);
        }
    }

    fn releasedir(
        &self,
        _req: &Request,
        ino: INodeNo,
        fh: FileHandle,
        _flags: OpenFlags,
        reply: ReplyEmpty,
    ) {
        if ino.0 == fh.0
            && let Ok(Entry {
                kind: EntryKind::Dir(_),
                ..
            }) = self.fs.as_rofs().entry(ino.0)
        {
            reply.ok();
        } else {
            reply.error(Errno::ESTALE);
        }
    }

    fn fsync(
        &self,
        _req: &Request,
        _ino: INodeNo,
        _fh: FileHandle,
        _datasync: bool,
        reply: ReplyEmpty,
    ) {
        reply.ok();
    }

    fn readdir(
        &self,
        _req: &Request,
        ino: INodeNo,
        _fh: FileHandle,
        offset: u64,
        mut reply: ReplyDirectory,
    ) {
        let Ok(Entry {
            kind: EntryKind::Dir(iter),
            ..
        }) = self.fs.as_rofs().entry(ino.0)
        else {
            reply.error(Errno::ENOENT);
            return;
        };

        for (entry, next) in iter.zip(1..).skip(offset as usize) {
            if reply.add(INodeNo(entry.inode), next, file_type(&entry), entry.name) {
                break;
            }
        }

        reply.ok();
    }

    fn readdirplus(
        &self,
        _req: &Request,
        ino: INodeNo,
        _fh: FileHandle,
        offset: u64,
        mut reply: ReplyDirectoryPlus,
    ) {
        let Ok(Entry {
            kind: EntryKind::Dir(iter),
            ..
        }) = self.fs.as_rofs().entry(ino.0)
        else {
            reply.error(Errno::ENOENT);
            return;
        };

        for (entry, next) in iter.zip(1..).skip(offset as usize) {
            let ino = INodeNo(entry.inode);
            let attr = self.file_attr(ino, &entry);

            if reply.add(ino, next, entry.name, &CACHE_TTL, &attr, Generation(0)) {
                break;
            }
        }

        reply.ok();
    }

    fn fsyncdir(
        &self,
        _req: &Request,
        _ino: INodeNo,
        _fh: FileHandle,
        _datasync: bool,
        reply: ReplyEmpty,
    ) {
        reply.ok();
    }

    fn statfs(&self, _req: &Request, _ino: INodeNo, reply: ReplyStatfs) {
        let files = self.fs.as_rofs().file_count() as u64;

        let blocks = self
            .bdts
            .0
            .values()
            .map(|bdt| u64::div_ceil(bdt.size, BLOCK_SIZE as u64))
            .sum();

        reply.statfs(blocks, 0, 0, files, 0, BLOCK_SIZE, MAX_NAME, BLOCK_SIZE);
    }

    fn getxattr(
        &self,
        _req: &Request,
        _ino: INodeNo,
        _name: &OsStr,
        _size: u32,
        reply: ReplyXattr,
    ) {
        reply.error(Errno::ENOTSUP);
    }

    fn access(&self, _req: &Request, _ino: INodeNo, mask: AccessFlags, reply: ReplyEmpty) {
        if !mask.contains(AccessFlags::W_OK) {
            reply.ok();
        } else {
            reply.error(Errno::EACCES);
        }
    }
}

fn file_type<T>(e: &Entry<'_, T>) -> FileType {
    match &e.kind {
        EntryKind::Dir(_) => FileType::Directory,
        EntryKind::File(_) => FileType::RegularFile,
    }
}

impl OnInterrupt for BackgroundSession {
    type Error = eyre::Error;

    fn on_interrupt(self) -> Result<(), Self::Error> {
        self.umount_and_join()?;
        Ok(())
    }
}

impl<F: DvdbndFilesystem> Deref for ArcDvdbndMount<F> {
    type Target = Arc<DvdbndMount<F>>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
