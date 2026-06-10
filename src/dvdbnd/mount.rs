use std::{cell::RefCell, ffi::OsStr, fs, io, path::Path, pin::pin, sync::Arc};

use color_eyre::eyre;
use compio::{buf::SetLen, dispatcher::Dispatcher, fs::File};
use futures_util::TryStreamExt;
use fxhash::{FxBuildHasher, FxHashMap};
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
            aligned::{self, AlignedBufferRef},
            encryption::{Ciphertext, CiphertextBuffer, EncryptionStore},
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
    fs: Arc<F>,
    dispatcher: Dispatcher,
    bdts: BdtTls,
    #[cfg(unix)]
    timestamp: std::time::SystemTime,
}

#[derive(Debug)]
struct BdtTls {
    paths: Box<[Box<Path>]>,
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
        let fs = Arc::new(fs);

        let dispatcher = Dispatcher::builder()
            .proactor_builder(aligned::proactor_builder())
            .build()?;

        let bdts = BdtTls::new(paths);

        Ok(Self {
            fs,
            dispatcher,
            bdts,
            #[cfg(unix)]
            timestamp: std::time::SystemTime::now(),
        })
    }

    async fn read_file(
        &self,
        inode: u64,
        file_offset: u64,
        len: u32,
        f: impl AsyncFn(&AlignedBufferRef) -> eyre::Result<()> + Send + 'static,
    ) -> eyre::Result<()> {
        let fs = self.fs.as_rofs();

        let file = match fs.entry(inode, true) {
            Ok(Entry::File(file)) => file,
            Ok(_) => return Err(RofsError::IsDir.into()),
            Err(e) => return Err(e.into()),
        };

        let file_len = file.len();
        let file_unpadded_len = file.unpadded_len();

        let file_offset = file_offset.min(file_len as u64);
        let len = len.min(file_len - file_offset as u32);

        let file_start = file.data_offset();
        let data_start = file_start + file_offset;

        let bdt = self.bdts.open(file.src_index()).await?;

        if let Some(encryption_index) = file.encryption_index() {
            return self
                .read_and_decrypt_file(
                    bdt,
                    data_start,
                    len,
                    file_start,
                    file_len,
                    file_unpadded_len,
                    encryption_index,
                    f,
                )
                .await;
        }

        let mut stream = pin!(aligned::stream_read(
            bdt, data_start, len, file_start, file_len
        ));

        while let Some(buf) = stream.try_next().await? {
            if !buf.0.is_empty() {
                f(&buf).await?;
            }
        }

        Ok(())
    }

    async fn read_and_decrypt_file(
        &self,
        bdt: File,
        data_start: u64,
        len: u32,
        file_start: u64,
        file_len: u32,
        file_unpadded_len: u32,
        encryption_index: usize,
        f: impl AsyncFn(&AlignedBufferRef) -> eyre::Result<()> + Send + 'static,
    ) -> eyre::Result<()> {
        let start_offset = (file_start - data_start) as u32;

        let fs = self.fs.clone();

        let encryption_store = fs.encryption_store();

        let mut stream = pin!(aligned::stream_read_context(
            bdt, data_start, len, file_start, file_len
        ));

        let mut cbuf = CiphertextBuffer::default();

        let mut is_done = false;
        let mut is_last = false;

        while !is_done {
            if is_last {
                is_done = true;
                cbuf.finish();
            } else if let Some(buf) = stream.try_next().await? {
                let file_offset = buf.1;

                let is_interesting = start_offset <= file_offset && file_offset < len;
                let is_first = cbuf.is_empty();

                cbuf.push(buf);

                if !is_interesting || is_first {
                    continue;
                }
            } else {
                is_last = true;
            }

            let ciphertext = Ciphertext::from_buffer(&mut cbuf);

            encryption_store
                .decrypt(encryption_index, ciphertext)?
                .await?;

            let buf = cbuf.curr();

            if let (buffer, file_offset) = buf
                && let buffer_end = *file_offset + buffer.len() as u32
                && let Some(padding) = (buffer_end).checked_sub(file_unpadded_len)
            {
                let padded_len = buffer.len();
                let unpadded_len = padded_len.saturating_sub(padding as usize);

                unsafe {
                    buf.0.set_len(unpadded_len);
                }
            }

            if !buf.0.is_empty() {
                f(buf).await?;
            }
        }

        Ok(())
    }
}

impl BdtTls {
    fn new<P>(paths: P) -> Self
    where
        P: IntoIterator<Item: AsRef<Path>>,
    {
        let paths = paths
            .into_iter()
            .map(|path| Box::from(path.as_ref()))
            .collect::<Vec<_>>()
            .into_boxed_slice();

        Self { paths }
    }

    async fn open(&self, bdt_index: usize) -> io::Result<File> {
        thread_local! {
            static MAP: RefCell<FxHashMap<Box<Path>, File>> =
                const { RefCell::new(FxHashMap::with_hasher(FxBuildHasher::new())) };
        }

        let path = &self.paths[bdt_index];

        if let Some(file) = MAP.with_borrow(|map| map.get(path).cloned()) {
            return Ok(file);
        }

        let new = File::open(path).await?;
        let file =
            MAP.with_borrow_mut(|map| map.entry(path.clone()).insert_entry(new).get().clone());

        Ok(file)
    }
}
