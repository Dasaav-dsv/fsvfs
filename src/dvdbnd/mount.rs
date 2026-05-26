use core::slice;
use std::{
    ffi::OsStr,
    fs,
    path::Path,
    sync::atomic::{AtomicU8, Ordering},
};

use color_eyre::eyre;
use memmap2::{MmapOptions, MmapRaw};
use parking_lot::Mutex;
use thiserror::Error;

use crate::{
    dvdbnd::{
        bhd5::FileAny,
        dict::Dictionary,
        filesystem::{
            DvdbndFile, DvdbndFilesystem, DvdbndRofs, DvdbndRofsBuilder,
            encryption::{DecryptError, EncryptionStore},
        },
        keys::Keys,
    },
    filesystem::readonly::{Entry, ReadOnlyFilesystem, RofsError},
};

#[derive(Debug, Error)]
pub enum ReadError {
    #[error(transparent)]
    Rofs(#[from] RofsError),

    #[error(transparent)]
    Decrypt(#[from] DecryptError),
}

#[derive(Debug)]
pub struct DvdbndMount<F: DvdbndFilesystem> {
    mmaps: Box<[MmapRaw]>,
    locks: Box<[FileLock]>,
    fs: F,
}

#[derive(Debug)]
struct FileLock {
    mutex: Mutex<()>,
    state: AtomicU8,
}

impl DvdbndMount<DvdbndRofs> {
    pub fn from_keys_and_dict(keys: &Keys<'_>, dict: Option<&Dictionary>) -> eyre::Result<Self> {
        let files = keys
            .by_path
            .iter()
            .map(|(path, key)| {
                let mut bytes = fs::read(path)?;
                if let Some(key) = key {
                    let len = key.decrypt_blocks_in_place(&mut bytes)?;
                    bytes.truncate(len);
                }
                Ok(bytes)
            })
            .collect::<eyre::Result<Vec<_>>>()?;

        let bhds = keys
            .by_path
            .iter()
            .zip(&files)
            .map(|((path, _), bytes)| {
                let file = FileAny::try_ref_from_bytes(&bytes)?;

                // FIXME
                let name = path
                    .file_prefix()
                    .and_then(OsStr::to_str)
                    .unwrap_or_default();

                Ok((name, file))
            })
            .collect::<eyre::Result<Vec<_>>>()?;

        let fs = DvdbndRofsBuilder::new()
            .with_bhds(bhds)
            .with_dict(dict)
            .finish()?;

        let bdts = keys.by_path.keys().map(|path| path.to_bdt());

        Self::from_fs_and_bdts(fs, bdts)
    }
}

impl<F: DvdbndFilesystem> DvdbndMount<F> {
    fn from_fs_and_bdts<P>(fs: F, paths: P) -> eyre::Result<Self>
    where
        P: IntoIterator<Item: AsRef<Path>>,
    {
        let mmaps = paths
            .into_iter()
            .map(|path| {
                let bdt = fs::File::open(path)?;
                let mmap_mut = unsafe { MmapOptions::new().map_copy(&bdt)? };
                Ok(MmapRaw::from(mmap_mut))
            })
            .collect::<eyre::Result<Vec<_>>>()?;

        let locks = fs
            .filesystem()
            .file_data_iter()
            .map(|file| match file.encryption_index() {
                Some(_) => FileLock::new(FileLock::IS_ENCRYPTED),
                None => FileLock::new(0),
            })
            .collect::<Vec<_>>();

        Ok(Self {
            mmaps: mmaps.into_boxed_slice(),
            locks: locks.into_boxed_slice(),
            fs,
        })
    }

    pub fn read(&self, path: &str) -> Result<&[u8], ReadError> {
        let fs = self.fs.filesystem();

        let inode = fs.lookup(path)?;
        let Entry::File(file) = fs.entry(inode)? else {
            return Err(RofsError::IsDir.into());
        };

        let mmap = &self.mmaps[file.src_index()];

        let data_offset = usize::try_from(file.data_offset()).unwrap();
        let file_len = match file.unpadded_len() {
            0 => file.len(),
            len => len,
        } as usize;

        if let Some(encryption_index) = file.encryption_index()
            && let lock = &self.locks[inode as usize]
            && lock.has_encrypted_flag()
        {
            let _guard = lock.mutex.lock();

            if lock.has_encrypted_flag() {
                let encryption_store = self.fs.encryption_store();

                let bytes = unsafe {
                    slice::from_raw_parts_mut(mmap.as_mut_ptr().add(data_offset), file_len)
                };

                encryption_store.decrypt(encryption_index, bytes)?;

                lock.clear_encrypted_flag();
            }
        }

        unsafe {
            Ok(slice::from_raw_parts(
                mmap.as_ptr().add(data_offset),
                file_len,
            ))
        }
    }
}

impl FileLock {
    const IS_ENCRYPTED: u8 = 1;

    const fn new(state: u8) -> Self {
        Self {
            mutex: Mutex::new(()),
            state: AtomicU8::new(state),
        }
    }

    fn has_encrypted_flag(&self) -> bool {
        self.state.load(Ordering::Acquire) & Self::IS_ENCRYPTED != 0
    }

    fn clear_encrypted_flag(&self) {
        self.state.fetch_and(!Self::IS_ENCRYPTED, Ordering::Release);
    }
}

impl Default for FileLock {
    fn default() -> Self {
        Self::new(0)
    }
}
