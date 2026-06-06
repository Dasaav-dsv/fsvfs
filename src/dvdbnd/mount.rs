use core::slice;
use std::{
    ffi::OsStr,
    fs,
    io::{Seek, SeekFrom},
    path::Path,
    ptr::NonNull,
    sync::atomic::{AtomicU8, AtomicU32, Ordering},
    time::SystemTime,
};

use color_eyre::eyre::{self, Context};
use memmap2::{MmapOptions, MmapRaw};
use parking_lot::Mutex;
use rayon::{
    ThreadPoolBuilder,
    iter::{IntoParallelRefIterator, ParallelIterator},
};
use sharded_slab::Slab;
use thiserror::Error;
use tracing::info;

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
    time::time,
};

#[cfg(unix)]
mod fuser;
#[cfg(windows)]
mod projfs;

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
    readers: Slab<FileReader>,
    timestamp: SystemTime,
    fs: F,
}

#[derive(Debug)]
struct Bdt {
    mmap: MmapRaw,
    size: usize,
}

#[derive(Debug)]
struct FileLock {
    mutex: Mutex<()>,
    state: AtomicU8,
}

#[derive(Debug)]
struct FileReader {
    ptr: NonNull<u8>,
    pos: AtomicU32,
    len: u32,
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
            .as_rofs()
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
            readers: Slab::new(),
            timestamp,
            fs,
        })
    }

    fn make_reader(&self, inode: u64) -> Result<FileReader, MakeReaderError> {
        let fs = self.fs.as_rofs();

        let entry = fs.entry(inode, true)?;
        let index = fs.file_index(&entry)?;

        let Entry::File(file) = entry else {
            return Err(RofsError::IsDir.into());
        };

        let mmap = &self.bdts[file.src_index()].mmap;

        let data_offset = usize::try_from(file.data_offset()).unwrap();
        let ptr = unsafe { mmap.as_mut_ptr().add(data_offset) };

        if let Some(encryption_index) = file.encryption_index()
            && let lock = &self.locks[index]
            && lock.has_encrypted_flag()
        {
            let _guard = lock.mutex.lock();

            if lock.has_encrypted_flag() {
                let file_len = file.len() as usize;

                self.fs
                    .encryption_store()
                    .decrypt(encryption_index, unsafe {
                        slice::from_raw_parts_mut(ptr, file_len)
                    })?;

                lock.clear_encrypted_flag();
            }
        }

        let file_len = match file.unpadded_len() {
            0 => file.len(),
            len => len,
        };

        Ok(FileReader {
            ptr: NonNull::new(ptr).unwrap(),
            pos: AtomicU32::new(0),
            len: file_len,
        })
    }

    fn open(&self, inode: u64) -> Result<usize, MakeReaderError> {
        let reader = self.make_reader(inode)?;
        self.readers
            .insert(reader)
            .ok_or(MakeReaderError::TooManyReaders)
    }

    fn read<'a>(&'a self, reader: &FileReader, len: u32) -> &'a [u8] {
        unsafe { reader.read(len).as_ref() }
    }

    fn read_mut<'a>(&'a self, reader: &mut FileReader, len: u32) -> &'a [u8] {
        unsafe { reader.read_mut(len).as_ref() }
    }

    fn seek(&self, reader: &FileReader, pos: SeekFrom) -> u64 {
        reader.seek(pos)
    }

    fn seek_mut(&self, reader: &mut FileReader, pos: SeekFrom) -> u64 {
        reader.seek_mut(pos)
    }
}

impl Bdt {
    fn open<P: AsRef<Path>>(path: P) -> eyre::Result<Self> {
        let mut file = fs::File::open(&path)?;

        let actual_size = file.seek(SeekFrom::End(0))?;
        let size = usize::try_from(actual_size).wrap_err("file is too large")?;

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
    fn read(&self, len: u32) -> NonNull<[u8]> {
        let mut read = 0;
        let self_pos = self.pos.update(Ordering::AcqRel, Ordering::Acquire, |pos| {
            let avail = self.len.saturating_sub(pos);
            read = len.min(avail);
            pos + read
        });

        let ptr = unsafe { self.ptr.add(self_pos as usize) };

        NonNull::slice_from_raw_parts(ptr, read as usize)
    }

    fn read_mut(&mut self, len: u32) -> NonNull<[u8]> {
        let self_pos = self.pos.get_mut();
        let pos = *self_pos;

        let avail = self.len.saturating_sub(pos);
        let read = len.min(avail);

        *self_pos += read;

        let ptr = unsafe { self.ptr.add(pos as usize) };

        NonNull::slice_from_raw_parts(ptr, read as usize)
    }

    fn seek(&self, pos: SeekFrom) -> u64 {
        let mut new_pos = 0;
        self.pos
            .update(Ordering::AcqRel, Ordering::Acquire, |self_pos| {
                new_pos = match pos {
                    SeekFrom::Start(offset) => offset,
                    SeekFrom::End(offset) => (self.len as u64).wrapping_add_signed(offset),
                    SeekFrom::Current(offset) => (self_pos as u64).wrapping_add_signed(offset),
                };
                new_pos as u32
            });

        new_pos
    }

    fn seek_mut(&mut self, pos: SeekFrom) -> u64 {
        let self_pos = self.pos.get_mut();

        let new_pos = match pos {
            SeekFrom::Start(offset) => offset,
            SeekFrom::End(offset) => (self.len as u64).wrapping_add_signed(offset),
            SeekFrom::Current(offset) => (*self_pos as u64).wrapping_add_signed(offset),
        };

        *self_pos = new_pos as u32;

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
