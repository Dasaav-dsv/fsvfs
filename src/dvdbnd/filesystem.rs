use std::num::NonZero;

use color_eyre::eyre;
use fxhash::FxBuildHasher;
use indexmap::IndexMap;

use crate::{
    dvdbnd::{
        bhd5::{
            ByteOrderExt, FileAny as Bhd5FileAny,
            format::{Buckets, Encryption, File as Bhd5File, FileEntry as Bhd5Entry},
        },
        dict::Dictionary,
        filesystem::encryption::{EncryptionStore, store_encryption},
    },
    filesystem::readonly::{Config, Normalize, ReadOnlyFilesystem, Rofs, RofsBuilder},
};

pub mod encryption;

pub trait DvdbndFilesystem {
    fn filesystem(&self) -> &impl ReadOnlyFilesystem<File: DvdbndFile>;

    fn encryption_store(&self) -> &impl EncryptionStore;
}

pub trait DvdbndFile {
    fn src_index(&self) -> usize;

    fn data_offset(&self) -> u64;

    fn len(&self) -> u32;

    fn unpadded_len(&self) -> u32;

    fn encryption_index(&self) -> Option<usize>;
}

#[cfg_attr(feature = "rkyv", derive(rkyv::Archive, rkyv::Serialize))]
#[derive(Debug)]
pub struct DvdbndRofs {
    inner: Rofs<'static, File, DvdbndConfig>,
    encryption_store: Vec<u8>,
}

#[derive(Debug)]
pub struct DvdbndRofsBuilder<'a, 'b, 'c> {
    bhds: IndexMap<&'a str, Bhd5FileAny<'b>, FxBuildHasher>,
    dict: Option<&'c Dictionary>,
}

#[derive(Debug)]
struct DvdbndConfig;

#[cfg_attr(feature = "rkyv", derive(rkyv::Archive, rkyv::Serialize))]
#[derive(Clone, Copy, Debug)]
struct File {
    data_offset: u64,
    len: u32,
    unpadded_len: u32,
    src_index: u32,
    encryption_index: Option<NonZero<u32>>,
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
        let mut files_by_bhd = vec![];

        for ((&bnd_name, bhd), i) in self.bhds.iter().zip(0..) {
            let files = match bhd {
                Bhd5FileAny::LE(bhd) => self.process_bhd(bhd, bnd_name, i, &mut encryption_store),
                Bhd5FileAny::BE(bhd) => self.process_bhd(bhd, bnd_name, i, &mut encryption_store),
            };

            files_by_bhd.push((bnd_name, files?));
        }

        let hash_paths = files_by_bhd
            .iter()
            .flat_map(|(bnd, files)| {
                files.iter().map(move |(hash, _, file)| {
                    if *hash <= u32::MAX as u64 {
                        (format!(".{bnd}/{:02x}/{hash:08x}", hash >> 24), file)
                    } else {
                        (format!(".{bnd}/{:02x}/{hash:16x}", hash >> 56), file)
                    }
                })
            })
            .collect::<Vec<_>>();

        let link_paths = files_by_bhd
            .iter()
            .flat_map(|(_, files)| files.as_slice())
            .zip(&hash_paths)
            .filter_map(|((_, to, _), (from, _))| Some(from.as_str()).zip(*to))
            .collect::<Vec<_>>();

        let inner = RofsBuilder::new()
            .with_files(
                hash_paths
                    .iter()
                    .map(|(path, file)| (path.as_str(), **file)),
            )
            .with_links(link_paths)
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
    ) -> eyre::Result<Vec<(u64, Option<&'c str>, File)>> {
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
    ) -> eyre::Result<Vec<(u64, Option<&'c str>, File)>>
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
                let encryption_index = match encryption
                    .map(|encryption| store_encryption(entry, encryption, encryption_store))
                {
                    Some(index) => {
                        let index = NonZero::new(index? ^ u32::MAX as usize)
                            .unwrap()
                            .try_into()
                            .expect("too many encryption entries");

                        Some(index)
                    }
                    None => None,
                };

                let hash = entry.path_hash();

                let path = hashes.as_ref().and_then(|hashes| match hashes {
                    Hashes::U32(hashes) => hashes.get(&(hash as u32)).cloned(),
                    Hashes::U64(hashes) => hashes.get(&hash).cloned(),
                });

                let file = File {
                    data_offset: entry.file_offset(),
                    src_index,
                    len: entry.file_size(),
                    unpadded_len: entry.unpadded_file_size().map(NonZero::get).unwrap_or(0),
                    encryption_index,
                };

                Ok((hash, path, file))
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
    fn filesystem(&self) -> &impl ReadOnlyFilesystem<File: DvdbndFile> {
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
        self.unpadded_len
    }

    #[inline]
    fn encryption_index(&self) -> Option<usize> {
        self.encryption_index
            .map(|index| (index.get() ^ u32::MAX) as usize)
    }
}

#[cfg(feature = "rkyv")]
impl DvdbndFilesystem for ArchivedDvdbndRofs {
    #[inline]
    fn filesystem(&self) -> &impl ReadOnlyFilesystem<File: DvdbndFile> {
        &self.inner
    }

    #[inline]
    fn encryption_store(&self) -> &impl EncryptionStore {
        &self.encryption_store
    }
}

#[cfg(feature = "rkyv")]
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
        self.unpadded_len.to_native()
    }

    #[inline]
    fn encryption_index(&self) -> Option<usize> {
        self.encryption_index
            .as_ref()
            .map(|index| (index.get() ^ u32::MAX) as usize)
    }
}

impl Config for DvdbndConfig {
    const SEPARATORS: &[char] = &['/', '\\'];
    const NORMALIZATION: Normalize = Normalize::AsciiCase;
}
