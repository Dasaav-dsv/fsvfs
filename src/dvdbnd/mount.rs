use std::{cell::RefCell, fs, io, path::Path, pin::pin, sync::Arc};

use color_eyre::eyre;
use compio::{dispatcher::Dispatcher, fs::File};
use futures_util::TryStreamExt;
use fxhash::{FxBuildHasher, FxHashMap};
use rayon::{
    ThreadPoolBuilder,
    iter::{IntoParallelRefIterator, IntoParallelRefMutIterator, ParallelIterator},
};
use tracing::info;
use xxhash_rust::{const_xxh3, xxh3::xxh3_128_with_seed};

use crate::{
    cache::Cache,
    dvdbnd::{
        bhd5::Bhd5File,
        dict::Dictionary,
        filesystem::{
            DvdbndFile, DvdbndFilesystem, DvdbndRofs, DvdbndRofsBuilder, DvdbndRofsCached, SrcId,
            SrcIdMap, aligned,
            encryption::{BLOCK_SIZE, Ciphertext, CiphertextBuffer, EncryptionId, EncryptionStore},
        },
        keys::Keys,
    },
    filesystem::{Entry, EntryKind, ReadOnlyFilesystem, RofsError},
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
struct BdtTls(SrcIdMap<Bdt>);

#[derive(Debug)]
struct Bdt {
    path: Box<Path>,
    #[cfg_attr(windows, expect(unused))]
    size: u64,
}

impl DvdbndMount<DvdbndRofs> {
    pub fn from_keys_and_dict(
        keys: &Keys<'_>,
        dict: Option<&Dictionary>,
        cache: Cache,
    ) -> eyre::Result<Self> {
        let thread_pool = ThreadPoolBuilder::new().use_current_thread().build()?;
        thread_pool.in_place_scope_fifo(|_| -> eyre::Result<Self> {
            let mut files = time!(
                keys.by_path
                    .par_iter()
                    .map(|(&path, key)| {
                        let bytes = fs::read(path)?;

                        const SEED: u64 = const_xxh3::xxh3_64(env!("CARGO_PKG_VERSION").as_bytes());
                        let src_id = SrcId::from(xxh3_128_with_seed(&bytes, SEED));

                        Ok((path, bytes, src_id, key))
                    })
                    .collect::<eyre::Result<Vec<_>>>()?,
                |t| info!("read BHD5 files ({t:.02?})"),
            );

            let bdts = files
                .iter()
                .map(|(path, _, src_id, _)| {
                    let bdt = Bdt::new(path.to_bdt())?;
                    Ok((*src_id, bdt))
                })
                .collect::<io::Result<SrcIdMap<_>>>()
                .map(BdtTls)?;

            let mut cached = Vec::with_capacity(files.len());

            files.retain(|(_, _, src_id, _)| {
                match cache.get::<DvdbndRofsCached>("dvdbnd.rofs", src_id.as_ref()) {
                    Some(cache) => {
                        cached.push((*src_id, cache));
                        false
                    }
                    None => true,
                }
            });

            let bhds = time!(
                files
                    .par_iter_mut()
                    .map(|(path, bytes, src_id, key)| {
                        if let Some(key) = key {
                            let len = key.decrypt_blocks_in_place(bytes)?;
                            bytes.truncate(len);
                        }

                        let file = Bhd5File::try_ref_from_bytes(bytes)?;
                        Ok((*path, (file, *src_id)))
                    })
                    .collect::<eyre::Result<Vec<_>>>()?,
                |t| info!("decrypted and parsed BHD5 files ({t:.02?})")
            );

            let fs = time!(
                DvdbndRofsBuilder::default()
                    .with_bhds(bhds)
                    .with_dict(dict)
                    .with_cached(cache, cached)
                    .finish()?,
                |t| info!("built dvdbnd read-only filesystem ({t:.02?})"),
            );

            let mount = Self::from_fs_and_bdts(fs, bdts)?;

            Ok(mount)
        })
    }
}

impl<F: DvdbndFilesystem> DvdbndMount<F>
where
    F: Send + Sync + 'static,
{
    fn from_fs_and_bdts(fs: F, bdts: BdtTls) -> eyre::Result<Self> {
        let fs = Arc::new(fs);

        let dispatcher = Dispatcher::builder()
            .proactor_builder(aligned::proactor_builder())
            .build()?;

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
        mut f: impl FnMut(&[u8], u32) -> eyre::Result<()>,
    ) -> eyre::Result<()> {
        let fs = self.fs.as_rofs();

        let file = match fs.entry(inode) {
            Ok(Entry {
                kind: EntryKind::File(file),
                ..
            }) => file,
            Ok(_) => return Err(RofsError::IsDir.into()),
            Err(e) => return Err(e.into()),
        };

        let file_len = file.unpadded_len();

        let file_offset = file_offset.min(file_len as u64) as u32;
        let len = len.min(file_len - file_offset);

        let data_start = file.data_offset + file_offset as u64;

        let end = file_offset + len;

        let mut f = move |buffer: &[u8], file_offset: i64| -> eyre::Result<()> {
            let Ok(file_offset) = u32::try_from(file_offset) else {
                return Ok(());
            };

            let buffer_end = file_offset + buffer.len() as u32;

            let end = buffer_end.min(end).saturating_sub(file_offset);
            let buffer = &buffer[..end as usize];

            if !buffer.is_empty() {
                f(buffer, file_offset)
            } else {
                Ok(())
            }
        };

        let bdt = self.bdts.open(file).await?;

        if let Some(encryption_id) = file.encryption_id {
            return self
                .read_and_decrypt_file(file, bdt, data_start, len, encryption_id, f)
                .await;
        }

        let mut stream = pin!(aligned::stream_read(
            bdt,
            data_start,
            len,
            file.data_offset,
            file.len,
        ));

        while let Some((buffer, file_offset)) = stream.try_next().await? {
            f(&buffer, file_offset)?;
        }

        Ok(())
    }

    async fn read_and_decrypt_file(
        &self,
        file: &DvdbndFile,
        bdt: File,
        data_start: u64,
        len: u32,
        encryption_id: EncryptionId,
        mut f: impl FnMut(&[u8], i64) -> eyre::Result<()>,
    ) -> eyre::Result<()> {
        let start = (data_start - file.data_offset) as u32;
        let end = start + len;

        let encryption_store = self.fs.encryption_store(file.src_id);

        let mut stream =
            aligned::stream_read_windows(bdt, data_start, len, file.data_offset, file.len);

        let mut cbuf = CiphertextBuffer::default();
        let mut is_last = false;

        while !is_last {
            if let Some(buf) = stream.try_next().await? {
                let is_first = cbuf.is_empty();
                cbuf.push(buf);

                if is_first {
                    continue;
                }
            } else {
                is_last = true;
                cbuf.finish();
            }

            let &mut (ref mut buffer, file_offset) = cbuf.curr();

            if file_offset < start as i64 || file_offset >= end as i64 {
                continue;
            }

            let buffer_len = buffer.len() as u32;
            let buffer_end = file_offset as u32 + buffer_len;

            if let Some(overread) = buffer_end.checked_sub(end) {
                let requested_len = buffer_len.saturating_sub(overread);
                buffer.truncate(requested_len as usize + BLOCK_SIZE - 1);
            }

            encryption_store
                .decrypt(encryption_id, Ciphertext::from_buffer(&mut cbuf))
                .await?;

            let (buffer, file_offset) = cbuf.curr();

            f(buffer, *file_offset)?;
        }

        Ok(())
    }
}

impl BdtTls {
    async fn open(&self, file: &DvdbndFile) -> io::Result<File> {
        thread_local! {
            static MAP: RefCell<FxHashMap<Box<Path>, File>> =
                const { RefCell::new(FxHashMap::with_hasher(FxBuildHasher::new())) };
        }

        let path = &self.0[&file.src_id].path;

        if let Some(file) = MAP.with_borrow(|map| map.get(path).cloned()) {
            return Ok(file);
        }

        let new = File::open(path).await?;
        let file =
            MAP.with_borrow_mut(|map| map.entry(path.clone()).insert_entry(new).get().clone());

        Ok(file)
    }
}

impl Bdt {
    fn new<P: AsRef<Path>>(path: P) -> io::Result<Self> {
        let size = fs::metadata(&path)?.len();
        Ok(Self {
            path: Box::from(path.as_ref()),
            size,
        })
    }
}
