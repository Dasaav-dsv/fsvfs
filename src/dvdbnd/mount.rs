use std::{ffi::OsStr, fs, mem::ManuallyDrop, num::NonZero, path::Path};

use color_eyre::eyre;
use compio::{
    buf::{IoBuf, Slice, buf_try},
    dispatcher::Dispatcher,
    driver::{AsRawFd, RawFd},
    fs::File,
    io::AsyncReadAtExt,
    runtime::Runtime,
};
use futures_util::{StreamExt, TryStreamExt, stream};
use rayon::{
    ThreadPoolBuilder,
    iter::{IntoParallelRefIterator, ParallelIterator},
};
use tracing::info;

use crate::{
    dvdbnd::{
        bhd5::FileAny,
        dict::Dictionary,
        filesystem::{
            DvdbndFile, DvdbndFilesystem, DvdbndRofs, DvdbndRofsBuilder,
            encryption::EncryptionStore,
        },
        keys::Keys,
    },
    filesystem::readonly::{Entry, ReadOnlyFilesystem, RofsError},
    time::time,
};

#[cfg(unix)]
mod fuser;
#[cfg(windows)]
mod projfs;

#[derive(Debug)]
pub struct DvdbndMount<F: DvdbndFilesystem> {
    fs: F,
    bdts: Box<[Bdt]>,
    dispatcher: Dispatcher,
    #[cfg(unix)]
    timestamp: std::time::SystemTime,
}

#[derive(Debug)]
struct Bdt {
    file: RawFd,
}

impl DvdbndMount<DvdbndRofs> {
    pub fn from_keys_and_dict(keys: &Keys<'_>, dict: Option<&Dictionary>) -> eyre::Result<Self> {
        let thread_pool = ThreadPoolBuilder::new().use_current_thread().build()?;
        let files = time!(
            thread_pool.in_place_scope_fifo(|_| {
                keys.by_path
                    .par_iter()
                    .map(|(path, key)| {
                        let mut bytes = fs::read(path)?;
                        if let Some(key) = key {
                            let len = key.decrypt_blocks_in_place(&mut bytes)?;
                            bytes.truncate(len);
                        }
                        Ok(bytes)
                    })
                    .collect::<eyre::Result<Vec<_>>>()
            })?,
            |t| info!("decrypted BHD5 files ({t:.02?})"),
        );

        let bhds = time!(
            keys.by_path
                .iter()
                .zip(&files)
                .map(|((path, _), bytes)| {
                    let file = FileAny::try_ref_from_bytes(bytes)?;

                    // FIXME
                    let name = path
                        .file_prefix()
                        .and_then(OsStr::to_str)
                        .unwrap_or_default();

                    Ok((name, file))
                })
                .collect::<eyre::Result<Vec<_>>>()?,
            |t| info!("parsed BHD5 files ({t:.02?})")
        );

        let fs = time!(
            DvdbndRofsBuilder::new()
                .with_bhds(bhds)
                .with_dict(dict)
                .finish()?,
            |t| info!("built dvdbnd read-only filesystem ({t:.02?})"),
        );

        let bdts = keys.by_path.keys().map(|path| path.to_bdt());

        let mount = Runtime::new()?.block_on(Self::from_fs_and_bdts(fs, bdts))?;

        Ok(mount)
    }
}

impl<F: DvdbndFilesystem> DvdbndMount<F>
where
    F: Send + Sync + 'static,
{
    async fn from_fs_and_bdts<P>(fs: F, paths: P) -> eyre::Result<Self>
    where
        P: IntoIterator<Item: AsRef<Path>>,
    {
        let bdts = stream::iter(paths)
            .then(Bdt::open)
            .try_collect::<Vec<_>>()
            .await?;

        let dispatcher = Dispatcher::new()?;

        Ok(Self {
            fs,
            bdts: bdts.into_boxed_slice(),
            dispatcher,
            #[cfg(unix)]
            timestamp: std::time::SystemTime::now(),
        })
    }

    async fn read_file(
        &self,
        inode: u64,
        alignment: Option<NonZero<usize>>,
    ) -> eyre::Result<Slice<Vec<u8>>> {
        let fs = self.fs.as_rofs();

        let file = match fs.entry(inode, true) {
            Ok(Entry::File(file)) => file,
            Ok(_) => return Err(RofsError::IsDir.into()),
            Err(e) => return Err(e.into()),
        };

        let src_index = file.src_index();
        let data_offset = file.data_offset();
        let file_len = file.len() as usize;

        let bdt = &self.bdts[src_index];

        let alignment = match alignment {
            Some(alignment) => alignment.get(),
            None => 512,
        };

        let buf_len = file_len + alignment - 1;
        let mut buf = Vec::<u8>::with_capacity(buf_len);

        let align_start = buf.as_ptr().align_offset(alignment);
        let align_end = align_start + file_len;

        buf.resize(align_start, 0);
        let slice = buf.slice(align_start..align_end);

        let bdt_file = unsafe { bdt.file() };
        let (_, mut slice) = buf_try!(
            @try bdt_file.read_exact_at(slice, data_offset).await
        );

        if slice.len() != file_len {
            return Err(eyre::eyre!("failed to read whole file"));
        }

        if let Some(index) = file.encryption_index() {
            let encryption_store = self.fs.encryption_store();
            encryption_store.decrypt(index, slice.as_inner_mut())?;
        }

        slice.set_end(match file.unpadded_len() {
            0 => file_len as usize,
            len => len as usize,
        });

        Ok(slice)
    }
}

impl Bdt {
    async fn open<P: AsRef<Path>>(path: P) -> eyre::Result<Self> {
        let file = ManuallyDrop::new(File::open(&path).await?).as_raw_fd();
        Ok(Self { file })
    }

    unsafe fn file(&self) -> ManuallyDrop<File> {
        cfg_select! {
            unix => unsafe {
                use compio::driver::FromRawFd;
                ManuallyDrop::new(File::from_raw_fd(self.file))
            }
            windows => unsafe {
                use std::os::windows::prelude::FromRawHandle;
                ManuallyDrop::new(File::from_raw_handle(self.file))
            },
        }
    }
}

unsafe impl Send for Bdt {}

unsafe impl Sync for Bdt {}
