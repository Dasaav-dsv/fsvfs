use std::{ffi::OsStr, fs, io, mem::ManuallyDrop, num::NonZero, path::Path};

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
    dispatcher: Dispatcher,
    bdt_reader: BdtReader,
    #[cfg(unix)]
    timestamp: std::time::SystemTime,
}

#[derive(Debug)]
struct BdtReader {
    bdts: Box<[Bdt]>,
    dispatcher: Dispatcher,
}

#[derive(Clone, Copy, Debug)]
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
        let mount = Self::from_fs_and_bdts(fs, bdts)?;

        Ok(mount)
    }
}

impl<F: DvdbndFilesystem> DvdbndMount<F>
where
    F: Send + Sync + 'static,
{
    fn from_fs_and_bdts<P>(fs: F, paths: P) -> eyre::Result<Self>
    where
        P: IntoIterator<Item: AsRef<Path>>,
    {
        let dispatcher = Dispatcher::new()?;
        let bdt_reader = BdtReader::from_bdts(paths)?;

        Ok(Self {
            fs,
            dispatcher,
            bdt_reader,
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

        const DEFAULT_BLOCK_ALIGNMENT: NonZero<usize> =
            const { NonZero::<usize>::new(4096).unwrap() };

        let alignment = alignment.unwrap_or(DEFAULT_BLOCK_ALIGNMENT);
        let encryption_store = self.fs.encryption_store();

        let slice = self
            .bdt_reader
            .read_file(file, alignment, encryption_store)
            .await?;

        Ok(slice)
    }
}

impl BdtReader {
    fn from_bdts<P>(paths: P) -> eyre::Result<Self>
    where
        P: IntoIterator<Item: AsRef<Path>>,
    {
        let dispatcher = Dispatcher::builder()
            .worker_threads(NonZero::<usize>::MIN)
            .build()?;

        let paths = paths
            .into_iter()
            .map(|path| path.as_ref().to_path_buf())
            .collect::<Vec<_>>();

        let bdts = Runtime::new()?.block_on(async {
            dispatcher
                .dispatch(move || stream::iter(paths).then(Bdt::open).try_collect::<Vec<_>>())?
                .await?
        })?;

        Ok(Self {
            bdts: bdts.into_boxed_slice(),
            dispatcher,
        })
    }

    async fn read_file<F: DvdbndFile>(
        &self,
        file: &F,
        alignment: NonZero<usize>,
        encryption_store: &impl EncryptionStore,
    ) -> eyre::Result<Slice<Vec<u8>>> {
        let alignment = alignment.get();
        let src_index = file.src_index();
        let data_offset = file.data_offset();
        let file_len = file.len() as usize;

        let buf_len = file_len + alignment - 1;
        let mut buf = Vec::<u8>::with_capacity(buf_len);

        let align_start = buf.as_ptr().align_offset(alignment);
        let align_end = align_start + file_len;

        buf.resize(align_start, 0);
        let mut slice = buf.slice(align_start..align_end);

        let bdt = self.bdts[src_index];
        slice = self
            .dispatcher
            .dispatch(async move || {
                // SAFETY: this is the thread this file is attached to.
                let bdt_file = unsafe { bdt.as_file() };
                let (_, slice) = buf_try!(
                    @try bdt_file.read_exact_at(slice, data_offset).await
                );
                io::Result::Ok(slice)
            })?
            .await??;

        if slice.len() != file_len {
            return Err(eyre::eyre!("failed to read whole file"));
        }

        if let Some(index) = file.encryption_index() {
            encryption_store
                .decrypt(index, slice.as_inner_mut())?
                .await?;
        }

        let actual_file_len = match file.unpadded_len() {
            0 => file_len as usize,
            len => len as usize,
        };

        slice.set_end(align_start + actual_file_len);

        Ok(slice)
    }
}

impl Bdt {
    async fn open<P: AsRef<Path>>(path: P) -> eyre::Result<Self> {
        let file = ManuallyDrop::new(File::open(&path).await?).as_raw_fd();
        Ok(Self { file })
    }

    unsafe fn as_file(&self) -> ManuallyDrop<File> {
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
