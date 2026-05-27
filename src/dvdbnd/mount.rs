use core::slice;
use std::{
    ffi::OsStr,
    fs,
    io::{Seek, SeekFrom},
    path::Path,
    ptr::NonNull,
    sync::atomic::{AtomicU8, Ordering},
    time::SystemTime,
};

use color_eyre::eyre;
use memmap2::{MmapOptions, MmapRaw};
use parking_lot::Mutex;
use sharded_slab::Pool;
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

#[cfg(unix)]
mod fuser;
#[cfg(windows)]
mod winfsp;

#[derive(Debug, Error)]
pub enum MakeReaderError {
    #[error(transparent)]
    Rofs(#[from] RofsError),

    #[error(transparent)]
    Decrypt(#[from] DecryptError),

    #[error("too many readers")]
    TooManyReaders,
}

#[derive(Debug)]
pub struct DvdbndMount<F: DvdbndFilesystem> {
    bdts: Box<[Bdt]>,
    locks: Box<[FileLock]>,
    readers: Pool<Option<Mutex<FileReader>>>,
    timestamp: SystemTime,
    fs: F,
}

#[derive(Debug)]
struct Bdt {
    mmap: MmapRaw,
    size: u64,
}

#[derive(Debug)]
struct FileLock {
    mutex: Mutex<()>,
    state: AtomicU8,
}

#[derive(Debug)]
struct FileReader {
    ptr: NonNull<u8>,
    len: u32,
    pos: u32,
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
    pub fn count_files(&self) -> usize {
        self.locks.len()
    }

    fn from_fs_and_bdts<P>(fs: F, paths: P) -> eyre::Result<Self>
    where
        P: IntoIterator<Item: AsRef<Path>>,
    {
        let bdts = paths
            .into_iter()
            .map(Bdt::open)
            .collect::<eyre::Result<Vec<_>>>()?;

        let locks = fs
            .filesystem()
            .file_iter()
            .map(|file| match file.encryption_index() {
                Some(_) => FileLock::new(FileLock::IS_ENCRYPTED),
                None => FileLock::new(0),
            })
            .collect::<Vec<_>>();

        let timestamp = SystemTime::now();

        Ok(Self {
            bdts: bdts.into_boxed_slice(),
            locks: locks.into_boxed_slice(),
            readers: Pool::new(),
            timestamp,
            fs,
        })
    }

    fn make_reader(&self, inode: u64) -> Result<usize, MakeReaderError> {
        let fs = self.fs.filesystem();

        let entry = fs.entry(inode, true)?;
        let index = fs.file_index(&entry)?;

        let Entry::File(file) = entry else {
            return Err(RofsError::IsDir.into());
        };

        let mmap = &self.bdts[file.src_index()].mmap;

        let data_offset = usize::try_from(file.data_offset()).unwrap();
        let ptr = unsafe { mmap.as_mut_ptr().add(data_offset) };
        let file_len = match file.unpadded_len() {
            0 => file.len(),
            len => len,
        };

        if let Some(encryption_index) = file.encryption_index()
            && let lock = &self.locks[index]
            && lock.has_encrypted_flag()
        {
            let _guard = lock.mutex.lock();

            if lock.has_encrypted_flag() {
                let encryption_store = self.fs.encryption_store();

                let bytes = unsafe { slice::from_raw_parts_mut(ptr, file_len as usize) };

                encryption_store.decrypt(encryption_index, bytes)?;

                lock.clear_encrypted_flag();
            }
        }

        let mut reader = self
            .readers
            .create()
            .ok_or(MakeReaderError::TooManyReaders)?;

        *reader = Some(Mutex::new(FileReader {
            ptr: NonNull::new(ptr).unwrap(),
            len: file_len,
            pos: 0,
        }));

        Ok(reader.key())
    }

    fn read<'a>(&'a self, reader: &mut FileReader, len: usize) -> &'a [u8] {
        unsafe { reader.read(len).as_ref() }
    }

    fn seek(&self, reader: &mut FileReader, pos: SeekFrom) -> u64 {
        reader.seek(pos)
    }
}

impl Bdt {
    fn open<P: AsRef<Path>>(path: P) -> eyre::Result<Self> {
        let mut file = fs::File::open(&path)?;

        let size = file.seek(SeekFrom::End(0))?;
        file.seek(SeekFrom::Start(0))?;

        let mmap = unsafe { MmapRaw::from(MmapOptions::new().map_copy(&file)?) };

        Ok(Self { mmap, size })
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

impl FileReader {
    fn read(&mut self, len: usize) -> NonNull<[u8]> {
        let avail = self.len.saturating_sub(self.pos);
        let read = len.min(avail as usize);

        let ptr = unsafe { self.ptr.add(self.pos as usize) };
        self.pos = self.pos.wrapping_add(read as u32);

        NonNull::slice_from_raw_parts(ptr, read)
    }

    fn seek(&mut self, pos: SeekFrom) -> u64 {
        let new_pos = match pos {
            SeekFrom::Start(offset) => offset,
            SeekFrom::End(offset) => (self.len as u64).wrapping_add_signed(offset),
            SeekFrom::Current(offset) => (self.pos as u64).wrapping_add_signed(offset),
        };

        self.pos = new_pos as u32;

        new_pos
    }
}

impl Default for FileLock {
    fn default() -> Self {
        Self::new(0)
    }
}

unsafe impl Send for FileReader {}

unsafe impl Sync for FileReader {}
