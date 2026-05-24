use std::{borrow::Cow, mem, num::NonZero};

use color_eyre::eyre;
use fxhash::FxBuildHasher;
use indexmap::IndexMap;

use crate::{
    cow::CowExt,
    dvdbnd::{
        bhd5::{
            ByteOrderExt,
            format::{Buckets, Encryption, File as Bhd5File, FileEntry as Bhd5Entry},
        },
        dict::Dictionary,
        filesystem::encryption::{EncryptionStore, store_encryption},
    },
    filesystem::readonly::{Config, ReadOnlyFilesystem, Rofs, RofsBuilder},
};

pub mod encryption;

pub trait BndFilesystem {
    fn filesystem(&self) -> &impl ReadOnlyFilesystem<File: BndFile>;

    fn encryption_store(&self) -> &impl EncryptionStore;
}

pub trait BndFile {
    fn data_index(&self) -> usize;

    fn data_offset(&self) -> u64;

    fn data_len(&self) -> u32;

    fn unpadded_data_len(&self) -> u32;

    fn encryption_index(&self) -> Option<usize>;
}

#[cfg_attr(feature = "rkyv", derive(rkyv::Archive, rkyv::Serialize))]
#[derive(Debug)]
pub struct BndRofs {
    inner: Rofs<'static, File, BndConfig>,
    encryption_store: Vec<u8>,
}

#[derive(Debug)]
pub struct BndRofsBuilder<'a, 'b, 'c, O: ByteOrderExt> {
    bhds: IndexMap<&'a str, Bhd5File<'b, O>, FxBuildHasher>,
    dict: Option<&'c Dictionary>,
}

#[derive(Debug)]
struct BndConfig;

#[cfg_attr(feature = "rkyv", derive(rkyv::Archive, rkyv::Serialize))]
#[derive(Clone, Copy, Debug)]
struct File {
    data_offset: u64,
    data_index: u32,
    data_len: u32,
    unpadded_data_len: u32,
    encryption_index: Option<NonZero<u32>>,
}

impl<'a, 'b, 'c, O: ByteOrderExt> BndRofsBuilder<'a, 'b, 'c, O> {
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
        I: IntoIterator<Item = (&'a str, Bhd5File<'b, O>)>,
    {
        self.bhds.extend(iter);
        self
    }

    pub fn finish(&mut self) -> eyre::Result<BndRofs> {
        let builder = mem::take(self);

        let mut encryption_store = vec![];
        let mut files_by_bhd = vec![];

        for ((&bnd_name, bhd), i) in builder.bhds.iter().zip(0..) {
            let encryption = &bhd.encryption;
            let store = &mut encryption_store;

            use Buckets as B;
            let files = match &bhd.buckets {
                B::DarkSouls(e) => self.process_files(e, bnd_name, i, encryption, store),
                B::DarkSouls2(e) => self.process_files(e, bnd_name, i, encryption, store),
                B::DarkSouls3(e) => self.process_files(e, bnd_name, i, encryption, store),
                B::EldenRing(e) => self.process_files(e, bnd_name, i, encryption, store),
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

        let hard_link_paths = files_by_bhd
            .iter()
            .flat_map(|(_, files)| files.as_slice())
            .zip(&hash_paths)
            .filter_map(|((_, from, _), (to, _))| from.zip(Some(to)))
            .collect::<Vec<_>>();

        let inner = RofsBuilder::new()
            .with_files(
                hash_paths
                    .iter()
                    .map(|(path, file)| (path.as_str(), **file)),
            )
            .with_hard_links(
                hard_link_paths
                    .iter()
                    .map(|&(from, to)| (from, to.as_str())),
            )
            .finish();

        Ok(BndRofs {
            inner,
            encryption_store,
        })
    }

    fn process_files<E: Bhd5Entry<O>>(
        &self,
        entries: &[&[E]],
        bnd_name: &str,
        data_index: u32,
        encryption: &[Option<&Encryption<O>>],
        encryption_store: &mut Vec<u8>,
    ) -> eyre::Result<Vec<(u64, Option<&'c str>, File)>> {
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
                        let index = NonZero::new(index? ^ usize::MAX)
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
                    data_index,
                    data_len: entry.file_size(),
                    unpadded_data_len: entry.unpadded_file_size().map(NonZero::get).unwrap_or(0),
                    encryption_index,
                };

                Ok((hash, path, file))
            })
            .collect()
    }
}

impl<O: ByteOrderExt> Default for BndRofsBuilder<'_, '_, '_, O> {
    fn default() -> Self {
        Self::new()
    }
}

impl BndFilesystem for BndRofs {
    #[inline]
    fn filesystem(&self) -> &impl ReadOnlyFilesystem<File: BndFile> {
        &self.inner
    }

    #[inline]
    fn encryption_store(&self) -> &impl EncryptionStore {
        &self.encryption_store
    }
}

impl BndFile for File {
    #[inline]
    fn data_index(&self) -> usize {
        self.data_index as usize
    }

    #[inline]
    fn data_offset(&self) -> u64 {
        self.data_offset
    }

    #[inline]
    fn data_len(&self) -> u32 {
        self.data_len
    }

    #[inline]
    fn unpadded_data_len(&self) -> u32 {
        self.unpadded_data_len
    }

    #[inline]
    fn encryption_index(&self) -> Option<usize> {
        self.encryption_index
            .map(|index| (index.get() ^ u32::MAX) as usize)
    }
}

#[cfg(feature = "rkyv")]
impl BndFilesystem for ArchivedBndRofs {
    #[inline]
    fn filesystem(&self) -> &impl ReadOnlyFilesystem<File: BndFile> {
        &self.inner
    }

    #[inline]
    fn encryption_store(&self) -> &impl EncryptionStore {
        &self.encryption_store
    }
}

#[cfg(feature = "rkyv")]
impl BndFile for ArchivedFile {
    #[inline]
    fn data_index(&self) -> usize {
        self.data_index.to_native() as usize
    }

    #[inline]
    fn data_offset(&self) -> u64 {
        self.data_offset.to_native()
    }

    #[inline]
    fn data_len(&self) -> u32 {
        self.data_len.to_native()
    }

    #[inline]
    fn unpadded_data_len(&self) -> u32 {
        self.unpadded_data_len.to_native()
    }

    #[inline]
    fn encryption_index(&self) -> Option<usize> {
        self.encryption_index
            .as_ref()
            .map(|index| (index.get() ^ u32::MAX) as usize)
    }
}

impl Config for BndConfig {
    const SEPARATORS: &[char] = &['/', '\\'];

    fn normalize_component(component: &str) -> Cow<'_, str> {
        let mut component = Cow::Borrowed(component);
        Cow::make_ascii_lowercase(&mut component);
        component
    }
}
