use std::{borrow::Cow, num::NonZero};

use color_eyre::eyre;
use fxhash::FxBuildHasher;
use indexmap::IndexMap;
use rayon::iter::{IndexedParallelIterator, IntoParallelRefIterator, ParallelIterator};

use crate::{
    dvdbnd::{
        bhd5::{
            ByteOrderExt, FileAny as Bhd5FileAny,
            format::{Buckets, Encryption, File as Bhd5File, FileEntry as Bhd5Entry},
        },
        dict::Dictionary,
        filesystem::encryption::{EncryptionId, EncryptionStore, store_encryption},
    },
    filesystem::{Config, ReadOnlyFilesystem, Rofs, object::RofsObject},
};

pub mod aligned;
pub mod encryption;

pub trait DvdbndFilesystem {
    fn as_rofs(&self) -> &impl ReadOnlyFilesystem<File = DvdbndFile>;

    fn encryption_store(&self, src_index: usize) -> &impl EncryptionStore;
}

#[derive(Debug)]
pub struct DvdbndRofs {
    inner: Rofs<DvdbndFile, DvdbndConfig>,
    encryption_store: Box<[Box<[u8]>]>,
}

#[derive(Debug)]
pub struct DvdbndRofsBuilder<'a, 'b, 'c> {
    bhds: IndexMap<&'a str, Bhd5FileAny<'b>, FxBuildHasher>,
    dict: Option<&'c Dictionary>,
}

#[derive(Clone, Copy, Debug)]
pub struct DvdbndFile {
    data_offset: u64,
    len: u32,
    unpadded_len: u32,
    src_index: u32,
    encryption_id: Option<EncryptionId>,
}

#[derive(Debug)]
struct DvdbndConfig;

impl<'a, 'b, 'c> DvdbndRofsBuilder<'a, 'b, 'c> {
    pub const fn new() -> Self {
        Self {
            bhds: IndexMap::with_hasher(FxBuildHasher::new()),
            dict: None,
        }
    }

    pub fn with_dict(&mut self, dict: Option<&'c Dictionary>) -> &mut Self {
        self.dict = dict;
        self
    }

    pub fn with_bhds<I>(&mut self, iter: I) -> &mut Self
    where
        I: IntoIterator<Item = (&'a str, Bhd5FileAny<'b>)>,
    {
        self.bhds.extend(iter);
        self
    }

    pub fn finish(&mut self) -> eyre::Result<DvdbndRofs> {
        let files = self
            .bhds
            .par_iter()
            .enumerate()
            .map(|(i, (&bnd_name, bhd))| match bhd {
                Bhd5FileAny::LE(bhd) => self.process_bhd(bhd, bnd_name, i as u32),
                Bhd5FileAny::BE(bhd) => self.process_bhd(bhd, bnd_name, i as u32),
            })
            .collect::<eyre::Result<Vec<_>>>()?;

        let inner = files
            .par_iter()
            .map(|(files, _)| RofsObject::new(files.iter().map(|(path, file)| (path, *file))))
            .reduce(RofsObject::<_, DvdbndConfig>::default, RofsObject::merge)
            .into_rofs();

        let encryption_store = files
            .into_iter()
            .map(|(_, store)| store.into_boxed_slice())
            .collect::<Vec<_>>()
            .into_boxed_slice();

        Ok(DvdbndRofs {
            inner,
            encryption_store,
        })
    }

    fn process_bhd<O: ByteOrderExt>(
        &self,
        bhd: &Bhd5File<'_, O>,
        bnd_name: &str,
        src_index: u32,
    ) -> eyre::Result<(Vec<(Cow<'c, str>, DvdbndFile)>, Vec<u8>)> {
        let encryption = &bhd.encryption;

        match &bhd.buckets {
            Buckets::DarkSouls(e) => self.process_files(e, bnd_name, src_index, encryption),
            Buckets::DarkSouls2(e) => self.process_files(e, bnd_name, src_index, encryption),
            Buckets::DarkSouls3(e) => self.process_files(e, bnd_name, src_index, encryption),
            Buckets::EldenRing(e) => self.process_files(e, bnd_name, src_index, encryption),
        }
    }

    fn process_files<O, E>(
        &self,
        entries: &[&[E]],
        bnd_name: &str,
        src_index: u32,
        encryption: &[Option<&Encryption<O>>],
    ) -> eyre::Result<(Vec<(Cow<'c, str>, DvdbndFile)>, Vec<u8>)>
    where
        O: ByteOrderExt,
        E: Bhd5Entry<O>,
    {
        enum Hashes<T0, T1> {
            U32(T0),
            U64(T1),
        }

        let hashes = self.dict.map(|dict| match E::U64_HASH {
            false => Hashes::U32(dict.hash_paths32(bnd_name)),
            true => Hashes::U64(dict.hash_paths64(bnd_name)),
        });

        let mut encryption_store = vec![];

        let files = entries
            .iter()
            .cloned()
            .flatten()
            .zip(encryption)
            .map(|(entry, encryption)| {
                let encryption_id = match encryption {
                    Some(encryption) => {
                        Some(store_encryption(entry, encryption, &mut encryption_store)?)
                    }
                    None => None,
                };

                let hash = entry.path_hash();

                let path = hashes
                    .as_ref()
                    .and_then(|hashes| match hashes {
                        Hashes::U32(hashes) => hashes.get(&(hash as u32)).cloned(),
                        Hashes::U64(hashes) => hashes.get(&hash).cloned(),
                    })
                    .map_or_else(
                        || {
                            if hash <= u32::MAX as u64 {
                                Cow::Owned(format!(".{bnd_name}/{:02x}/{hash:08x}", hash >> 24))
                            } else {
                                Cow::Owned(format!(".{bnd_name}/{:02x}/{hash:16x}", hash >> 56))
                            }
                        },
                        Cow::Borrowed,
                    );

                let file = DvdbndFile {
                    data_offset: entry.file_offset(),
                    src_index,
                    len: entry.file_size(),
                    unpadded_len: entry.unpadded_file_size().map(NonZero::get).unwrap_or(0),
                    encryption_id,
                };

                Ok((path, file))
            })
            .collect::<eyre::Result<Vec<_>>>()?;

        Ok((files, encryption_store))
    }
}

impl Default for DvdbndRofsBuilder<'_, '_, '_> {
    fn default() -> Self {
        Self::new()
    }
}

impl DvdbndFilesystem for DvdbndRofs {
    #[inline]
    fn as_rofs(&self) -> &impl ReadOnlyFilesystem<File = DvdbndFile> {
        &self.inner
    }

    #[inline]
    fn encryption_store(&self, src_index: usize) -> &impl EncryptionStore {
        &self.encryption_store[src_index]
    }
}

impl DvdbndFile {
    pub fn src_index(&self) -> usize {
        self.src_index as usize
    }

    pub fn data_offset(&self) -> u64 {
        self.data_offset
    }

    pub fn len(&self) -> u32 {
        self.len
    }

    pub fn unpadded_len(&self) -> u32 {
        match self.unpadded_len {
            0 => self.len(),
            len => len,
        }
    }

    pub fn encryption_id(&self) -> Option<EncryptionId> {
        self.encryption_id
    }
}

impl Config for DvdbndConfig {
    const SEPARATORS: &[char] = &['/', '\\'];
}
