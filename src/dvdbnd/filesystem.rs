use std::{borrow::Cow, num::NonZero};

use color_eyre::eyre;
use fxhash::FxBuildHasher;
use indexmap::IndexMap;
use rkyv::{Archive, Serialize};

use crate::{
    dvdbnd::{
        bhd5::{
            ByteOrderExt, FileAny as Bhd5FileAny,
            format::{Buckets, Encryption, File as Bhd5File, FileEntry as Bhd5Entry},
        },
        dict::Dictionary,
        filesystem::encryption::{
            ArchivedEncryptionId, EncryptionId, EncryptionStore, store_encryption,
        },
    },
    filesystem::{Config, ReadOnlyFilesystem, Rofs, RofsBuilder},
};

pub mod aligned;
pub mod encryption;

pub trait DvdbndFilesystem {
    fn as_rofs(&self) -> &impl ReadOnlyFilesystem<File: DvdbndFile>;

    fn encryption_store(&self) -> &impl EncryptionStore;
}

pub trait DvdbndFile {
    fn src_index(&self) -> usize;

    fn data_offset(&self) -> u64;

    fn len(&self) -> u32;

    fn unpadded_len(&self) -> u32;

    fn encryption_id(&self) -> Option<EncryptionId>;
}

#[derive(Debug, Archive, Serialize)]
pub struct DvdbndRofs {
    inner: Rofs<File, DvdbndConfig>,
    encryption_store: Vec<u8>,
}

#[derive(Debug)]
pub struct DvdbndRofsBuilder<'a, 'b, 'c> {
    bhds: IndexMap<&'a str, Bhd5FileAny<'b>, FxBuildHasher>,
    dict: Option<&'c Dictionary>,
}

#[derive(Debug)]
struct DvdbndConfig;

#[derive(Clone, Copy, Debug, Archive, Serialize)]
struct File {
    data_offset: u64,
    len: u32,
    unpadded_len: u32,
    src_index: u32,
    encryption_id: Option<EncryptionId>,
}

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
        let mut encryption_store = vec![];
        let mut files_by_path = vec![];

        for ((&bnd_name, bhd), i) in self.bhds.iter().zip(0..) {
            let files = match bhd {
                Bhd5FileAny::LE(bhd) => self.process_bhd(bhd, bnd_name, i, &mut encryption_store),
                Bhd5FileAny::BE(bhd) => self.process_bhd(bhd, bnd_name, i, &mut encryption_store),
            };

            files_by_path.push(files?);
        }

        let inner = RofsBuilder::new()
            .with_files(
                files_by_path
                    .iter()
                    .flatten()
                    .map(|(path, file)| (&**path, *file)),
            )
            .finish();

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
        encryption_store: &mut Vec<u8>,
    ) -> eyre::Result<Vec<(Cow<'c, str>, File)>> {
        let encryption = &bhd.encryption;

        let files = match &bhd.buckets {
            Buckets::DarkSouls(e) => {
                self.process_files(e, bnd_name, src_index, encryption, encryption_store)?
            }
            Buckets::DarkSouls2(e) => {
                self.process_files(e, bnd_name, src_index, encryption, encryption_store)?
            }
            Buckets::DarkSouls3(e) => {
                self.process_files(e, bnd_name, src_index, encryption, encryption_store)?
            }
            Buckets::EldenRing(e) => {
                self.process_files(e, bnd_name, src_index, encryption, encryption_store)?
            }
        };

        Ok(files)
    }

    fn process_files<O, E>(
        &self,
        entries: &[&[E]],
        bnd_name: &str,
        src_index: u32,
        encryption: &[Option<&Encryption<O>>],
        encryption_store: &mut Vec<u8>,
    ) -> eyre::Result<Vec<(Cow<'c, str>, File)>>
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

        entries
            .iter()
            .cloned()
            .flatten()
            .zip(encryption)
            .map(|(entry, encryption)| {
                let encryption_id = match encryption {
                    Some(encryption) => {
                        Some(store_encryption(entry, encryption, encryption_store)?)
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

                let file = File {
                    data_offset: entry.file_offset(),
                    src_index,
                    len: entry.file_size(),
                    unpadded_len: entry.unpadded_file_size().map(NonZero::get).unwrap_or(0),
                    encryption_id,
                };

                Ok((path, file))
            })
            .collect()
    }
}

impl Default for DvdbndRofsBuilder<'_, '_, '_> {
    fn default() -> Self {
        Self::new()
    }
}

impl DvdbndFilesystem for DvdbndRofs {
    #[inline]
    fn as_rofs(&self) -> &impl ReadOnlyFilesystem<File: DvdbndFile> {
        &self.inner
    }

    #[inline]
    fn encryption_store(&self) -> &impl EncryptionStore {
        &self.encryption_store
    }
}

impl DvdbndFile for File {
    #[inline]
    fn src_index(&self) -> usize {
        self.src_index as usize
    }

    #[inline]
    fn data_offset(&self) -> u64 {
        self.data_offset
    }

    #[inline]
    fn len(&self) -> u32 {
        self.len
    }

    #[inline]
    fn unpadded_len(&self) -> u32 {
        match self.unpadded_len {
            0 => self.len(),
            len => len,
        }
    }

    #[inline]
    fn encryption_id(&self) -> Option<EncryptionId> {
        self.encryption_id
    }
}

impl DvdbndFilesystem for ArchivedDvdbndRofs {
    #[inline]
    fn as_rofs(&self) -> &impl ReadOnlyFilesystem<File: DvdbndFile> {
        &self.inner
    }

    #[inline]
    fn encryption_store(&self) -> &impl EncryptionStore {
        &self.encryption_store
    }
}

impl DvdbndFile for ArchivedFile {
    #[inline]
    fn src_index(&self) -> usize {
        self.src_index.to_native() as usize
    }

    #[inline]
    fn data_offset(&self) -> u64 {
        self.data_offset.to_native()
    }

    #[inline]
    fn len(&self) -> u32 {
        self.len.to_native()
    }

    #[inline]
    fn unpadded_len(&self) -> u32 {
        match self.unpadded_len.to_native() {
            0 => self.len(),
            len => len,
        }
    }

    #[inline]
    fn encryption_id(&self) -> Option<EncryptionId> {
        self.encryption_id.as_ref().map(ArchivedEncryptionId::get)
    }
}

impl Config for DvdbndConfig {
    const SEPARATORS: &[char] = &['/', '\\'];
}
