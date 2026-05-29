use std::{
    ffi::OsStr,
    fs,
    io::{self, SeekFrom},
    sync::Arc,
    time::{Duration, SystemTime},
};

use color_eyre::eyre;
use fuser::{
    AccessFlags, Config, Errno, FileAttr, FileHandle, FileType, FopenFlags, Generation, INodeNo,
    KernelConfig, LockOwner, MountOption, OpenAccMode, OpenFlags, ReplyAttr, ReplyData,
    ReplyDirectory, ReplyDirectoryPlus, ReplyEmpty, ReplyEntry, ReplyLseek, ReplyOpen, ReplyStatfs,
    Request, spawn_mount2,
};
use libc::{SEEK_CUR, SEEK_END, SEEK_SET};
use parking_lot::{Condvar, Mutex};
use tracing::warn;

use crate::{
    dvdbnd::{
        filesystem::{DvdbndFile, DvdbndFilesystem},
        mount::{DvdbndMount, MakeReaderError},
    },
    filesystem::readonly::{Entry, ReadOnlyFilesystem},
};

const MAX_NAME: u32 = 255;
const BLOCK_SIZE: u32 = 512;

const CACHE_TTL: Duration = Duration::new(60, 0);

impl<F> DvdbndMount<F>
where
    F: DvdbndFilesystem + Send + Sync + 'static,
{
    pub fn mount(self, mountpoint: &str) -> eyre::Result<()> {
        fs::create_dir_all(mountpoint)?;

        let mut config = Config::default();
        config.mount_options = vec![MountOption::FSName("fsvfs".to_string()), MountOption::RO];

        #[cfg(target_os = "linux")]
        {
            config.clone_fd = true;
            config.n_threads = match std::thread::available_parallelism() {
                Ok(threads) => Some(threads.get().min(16)),
                Err(_) => None,
            };
        }

        let umount_res = Arc::new((Mutex::new(None), Condvar::new()));
        let mount = spawn_mount2(self, mountpoint, &config)?;

        ctrlc::set_handler({
            let umount_res = umount_res.clone();
            let mut mount = Some(mount);
            move || {
                if let Some(mount) = mount.take() {
                    let res = mount.umount_and_join();
                    let (lock, cvar) = &*umount_res;

                    let mut umount_res = lock.lock();
                    *umount_res = Some(res);

                    cvar.notify_all();
                }
            }
        })?;

        let (lock, cvar) = &*umount_res;
        let mut umount_res = lock.lock();
        loop {
            if let Some(res) = umount_res.take() {
                return Ok(res?);
            }

            cvar.wait(&mut umount_res);
        }
    }

    fn file_attr<T: DvdbndFile>(&self, ino: INodeNo, entry: &Entry<'_, T>) -> FileAttr {
        let kind = file_type(entry);

        let (size, blocks) = if let Entry::File(data) = entry {
            let size = data.len();
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

impl<F> fuser::Filesystem for DvdbndMount<F>
where
    F: DvdbndFilesystem + Send + Sync + 'static,
{
    fn init(&mut self, _req: &Request, config: &mut KernelConfig) -> io::Result<()> {
        self.timestamp = SystemTime::now();

        let _ = config.set_max_stack_depth(1);

        Ok(())
    }

    fn destroy(&mut self) {}

    fn lookup(&self, _req: &Request, parent: INodeNo, name: &OsStr, reply: ReplyEntry) {
        let fs = self.fs.filesystem();

        let Some(name) = name.to_str() else {
            reply.error(Errno::ENOSYS);
            return;
        };

        let Ok(parent) = fs.path(parent.0) else {
            reply.error(Errno::ENOENT);
            return;
        };

        let Ok(ino) = fs.lookup([parent, name]) else {
            reply.error(Errno::ENOENT);
            return;
        };

        let Ok(entry) = self.fs.filesystem().entry(ino, true) else {
            reply.error(Errno::ENOENT);
            return;
        };

        let attr = self.file_attr(INodeNo(ino), &entry);

        reply.entry(&CACHE_TTL, &attr, Generation(0));
    }

    fn getattr(&self, _req: &Request, ino: INodeNo, _fh: Option<FileHandle>, reply: ReplyAttr) {
        let Ok(entry) = self.fs.filesystem().entry(ino.0, true) else {
            reply.error(Errno::ENOENT);
            return;
        };

        let attr = self.file_attr(ino, &entry);

        reply.attr(&CACHE_TTL, &attr);
    }

    fn readlink(&self, _req: &Request, ino: INodeNo, reply: ReplyData) {
        let fs = self.fs.filesystem();

        let Ok(Entry::Link(ino)) = fs.entry(ino.0, false) else {
            reply.error(Errno::ENOENT);
            return;
        };

        let Ok(path) = fs.path(ino) else {
            reply.error(Errno::EBADF);
            return;
        };

        reply.data(&path.as_bytes());
    }

    fn open(&self, _req: &Request, ino: INodeNo, flags: OpenFlags, reply: ReplyOpen) {
        if flags.acc_mode() != OpenAccMode::O_RDONLY {
            reply.error(Errno::EROFS);
            return;
        }

        match self.make_reader(ino.0) {
            Ok(key) => {
                let fh = FileHandle(key as u64);
                reply.opened(fh, FopenFlags::FOPEN_DIRECT_IO | FopenFlags::FOPEN_NOFLUSH);
            }
            Err(e) => {
                match &e {
                    MakeReaderError::Rofs(_) => reply.error(Errno::ENOENT),
                    MakeReaderError::Decrypt(_) => reply.error(Errno::EBADF),
                    MakeReaderError::TooManyReaders => reply.error(Errno::EMFILE),
                }
                warn!("could not open file: {e}");
            }
        }
    }

    fn read(
        &self,
        _req: &Request,
        _ino: INodeNo,
        fh: FileHandle,
        offset: u64,
        size: u32,
        _flags: OpenFlags,
        _lock_owner: Option<LockOwner>,
        reply: ReplyData,
    ) {
        let Ok(key) = usize::try_from(fh.0) else {
            reply.error(Errno::ESTALE);
            return;
        };

        if let Some(reader) = self.readers.get(key)
            && let Some(reader) = reader.as_ref()
        {
            let mut reader = reader.lock();
            let reader = &mut *reader;

            self.seek(reader, SeekFrom::Start(offset));
            let data = self.read(reader, size as usize);

            reply.data(data);
        } else {
            reply.error(Errno::ESTALE);
        }
    }

    fn release(
        &self,
        _req: &Request,
        _ino: INodeNo,
        fh: FileHandle,
        _flags: OpenFlags,
        _lock_owner: Option<LockOwner>,
        _flush: bool,
        reply: ReplyEmpty,
    ) {
        let Ok(key) = usize::try_from(fh.0) else {
            reply.error(Errno::ESTALE);
            return;
        };

        if !self.readers.clear(key) {
            reply.error(Errno::ESTALE);
        } else {
            reply.ok();
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
        let fs = self.fs.filesystem();

        let Ok(Entry::Dir(range)) = fs.entry(ino.0, true) else {
            reply.error(Errno::ENOENT);
            return;
        };

        let iter = match fs.entries_iter(range.clone()) {
            Ok(iter) => iter,
            Err(e) => {
                warn!("got bad range: {e}");
                reply.error(Errno::EBADF);
                return;
            }
        };

        for ((ino, entry), next) in range.zip(iter).zip(1..).skip(offset as usize) {
            let Ok(path) = fs.path(ino) else {
                warn!("failed to retrieve path for {ino}");
                continue;
            };

            let name = match path.rsplit_once("/") {
                Some((_, name)) => name,
                None => path,
            };

            if reply.add(INodeNo(ino), next, file_type(&entry), name) {
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
        let fs = self.fs.filesystem();

        let Ok(Entry::Dir(range)) = fs.entry(ino.0, true) else {
            reply.error(Errno::ENOENT);
            return;
        };

        let iter = match fs.entries_iter(range.clone()) {
            Ok(iter) => iter,
            Err(e) => {
                warn!("got bad range: {e}");
                reply.error(Errno::EBADF);
                return;
            }
        };

        for ((ino, entry), next) in range.zip(iter).zip(1..).skip(offset as usize) {
            let Ok(path) = fs.path(ino) else {
                warn!("failed to retrieve path for {ino}");
                continue;
            };

            let name = match path.rsplit_once("/") {
                Some((_, name)) => name,
                None => path,
            };

            let ino = INodeNo(ino);
            let attr = self.file_attr(ino, &entry);

            if reply.add(ino, next, name, &CACHE_TTL, &attr, Generation(0)) {
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
        let files = self.count_files() as u64;

        let blocks = self
            .bdts
            .iter()
            .map(|bdt| bdt.size.div_ceil(BLOCK_SIZE as u64))
            .sum();

        reply.statfs(blocks, 0, 0, files, 0, BLOCK_SIZE, MAX_NAME, BLOCK_SIZE);
    }

    fn access(&self, _req: &Request, _ino: INodeNo, mask: AccessFlags, reply: ReplyEmpty) {
        if mask.contains(AccessFlags::W_OK) {
            reply.error(Errno::EACCES);
        } else {
            reply.ok();
        }
    }

    fn lseek(
        &self,
        _req: &Request,
        _ino: INodeNo,
        fh: FileHandle,
        offset: i64,
        whence: i32,
        reply: ReplyLseek,
    ) {
        let pos = match whence {
            SEEK_SET => SeekFrom::Start(offset as u64),
            SEEK_END => SeekFrom::End(offset),
            SEEK_CUR => SeekFrom::Current(offset),
            _ => {
                warn!("unsupported value of whence ({whence})");
                reply.error(Errno::ENOSYS);
                return;
            }
        };

        let Ok(key) = usize::try_from(fh.0) else {
            reply.error(Errno::ESTALE);
            return;
        };

        if let Some(reader) = self.readers.get(key)
            && let Some(reader) = reader.as_ref()
        {
            let mut reader = reader.lock();
            let reader = &mut *reader;

            let offset = self.seek(reader, pos);

            reply.offset(offset as i64);
        } else {
            reply.error(Errno::ESTALE);
        }
    }
}

fn file_type<T>(e: &Entry<'_, T>) -> FileType {
    match e {
        Entry::Dir(_) => FileType::Directory,
        Entry::File(_) => FileType::RegularFile,
        Entry::Link(_) => FileType::Symlink,
    }
}
