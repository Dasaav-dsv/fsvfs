use std::{
    borrow::Cow,
    collections::HashMap,
    ffi::OsStr,
    hash::{BuildHasherDefault, Hasher},
    num::NonZero,
};

use color_eyre::eyre;
use fxhash::{FxBuildHasher, FxHashMap};
use rayon::iter::{IntoParallelRefIterator, ParallelIterator};
use rkyv::{Archive, Deserialize, Portable, Serialize};

use crate::{
    dvdbnd::{
        bhd5::{
            self, Bhd5File, Bhd5FileKind, ByteOrderExt,
            format::{Buckets, Encryption, FileEntry as Bhd5Entry},
        },
        dict::Dictionary,
        filesystem::encryption::{EncryptionId, EncryptionStore, store_encryption},
        path::BhdPath,
    },
    filesystem::{Config, ReadOnlyFilesystem, Rofs, object::RofsObject},
};

pub mod aligned;
pub mod encryption;

pub trait DvdbndFilesystem {
    fn as_rofs(&self) -> &impl ReadOnlyFilesystem<File = DvdbndFile>;

    fn encryption_store(&self, src_id: SrcId) -> &impl EncryptionStore;
}

#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Archive,
    Serialize,
    Deserialize,
    Portable,
)]
#[repr(transparent)]
pub struct SrcId([u8; 12]);

#[derive(Default, Clone, Debug)]
#[repr(transparent)]
pub struct SrcIdHasher(u64);

pub type SrcIdMap<V> = HashMap<SrcId, V, BuildHasherDefault<SrcIdHasher>>;

#[derive(Debug)]
pub struct DvdbndRofs {
    inner: Rofs<DvdbndFile, DvdbndConfig>,
    encryption_store: SrcIdMap<Box<[u8]>>,
}

#[derive(Debug)]
pub struct DvdbndRofsBuilder<'a, 'b, 'c> {
    bhds: FxHashMap<&'a BhdPath, Bhd5File<'b>>,
    dict: Option<&'c Dictionary>,
}

#[derive(Clone, Copy, Debug, Archive, Serialize, Deserialize)]
pub struct DvdbndFile {
    pub data_offset: u64,
    pub len: u32,
    unpadded_len: u32,
    pub src_id: SrcId,
    pub encryption_id: Option<EncryptionId>,
}

#[derive(Debug)]
struct DvdbndConfig;

type ProcessedBhd<'a> = Vec<(Cow<'a, str>, DvdbndFile)>;

impl<'a, 'b, 'c> DvdbndRofsBuilder<'a, 'b, 'c> {
    pub const fn new() -> Self {
        Self {
            bhds: FxHashMap::with_hasher(FxBuildHasher::new()),
            dict: None,
        }
    }

    pub fn with_dict(&mut self, dict: Option<&'c Dictionary>) -> &mut Self {
        self.dict = dict;
        self
    }

    pub fn with_bhds<I>(&mut self, iter: I) -> &mut Self
    where
        I: IntoIterator<Item = (&'a BhdPath, Bhd5File<'b>)>,
    {
        self.bhds.extend(iter);
        self
    }

    pub fn finish(&mut self) -> eyre::Result<DvdbndRofs> {
        let files = self
            .bhds
            .par_iter()
            .map(|(&name, bhd)| match &bhd.kind {
                Bhd5FileKind::LE(kind) => self.process_bhd(kind, name, bhd.hash),
                Bhd5FileKind::BE(kind) => self.process_bhd(kind, name, bhd.hash),
            })
            .collect::<eyre::Result<Vec<_>>>()?;

        let inner = files
            .par_iter()
            .map(|(files, _, _)| RofsObject::new(files.iter().map(|(path, file)| (path, *file))))
            .reduce(RofsObject::<_, DvdbndConfig>::default, RofsObject::merge)
            .into_rofs();

        let encryption_store = files
            .into_iter()
            .map(|(_, store, src_id)| (src_id, store.into_boxed_slice()))
            .collect::<SrcIdMap<_>>();

        Ok(DvdbndRofs {
            inner,
            encryption_store,
        })
    }

    fn process_bhd<O: ByteOrderExt>(
        &self,
        bhd: &bhd5::format::File<'_, O>,
        path: &BhdPath,
        hash: u128,
    ) -> eyre::Result<(ProcessedBhd<'c>, Vec<u8>, SrcId)> {
        let name = path
            .file_prefix()
            .and_then(OsStr::to_str)
            .unwrap_or_default();

        let encryption = &bhd.encryption;

        match &bhd.buckets {
            Buckets::DarkSouls(e) => self.process_files(e, name, hash, encryption),
            Buckets::DarkSouls2(e) => self.process_files(e, name, hash, encryption),
            Buckets::DarkSouls3(e) => self.process_files(e, name, hash, encryption),
            Buckets::EldenRing(e) => self.process_files(e, name, hash, encryption),
        }
    }

    fn process_files<O, E>(
        &self,
        entries: &[&[E]],
        name: &str,
        hash: u128,
        encryption: &[Option<&Encryption<O>>],
    ) -> eyre::Result<(ProcessedBhd<'c>, Vec<u8>, SrcId)>
    where
        O: ByteOrderExt,
        E: Bhd5Entry<O>,
    {
        enum Hashes<T0, T1> {
            U32(T0),
            U64(T1),
        }

        let hashes = self.dict.map(|dict| match E::U64_HASH {
            false => Hashes::U32(dict.hash_paths32(name)),
            true => Hashes::U64(dict.hash_paths64(name)),
        });

        let src_id = SrcId::from(hash);
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
                                Cow::Owned(format!(".{name}/{:02x}/{hash:08x}", hash >> 24))
                            } else {
                                Cow::Owned(format!(".{name}/{:02x}/{hash:16x}", hash >> 56))
                            }
                        },
                        Cow::Borrowed,
                    );

                let file = DvdbndFile {
                    data_offset: entry.file_offset(),
                    src_id,
                    len: entry.file_size(),
                    unpadded_len: entry.unpadded_file_size().map(NonZero::get).unwrap_or(0),
                    encryption_id,
                };

                Ok((path, file))
            })
            .collect::<eyre::Result<Vec<_>>>()?;

        Ok((files, encryption_store, src_id))
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
    fn encryption_store(&self, id: SrcId) -> &impl EncryptionStore {
        &self.encryption_store[&id]
    }
}

impl From<u128> for SrcId {
    fn from(value: u128) -> Self {
        Self(*value.to_be_bytes()[..12].as_array().unwrap())
    }
}

impl Hasher for SrcIdHasher {
    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        self.0 = u64::from_be_bytes(*bytes[..8].as_array().unwrap())
    }

    #[inline]
    fn finish(&self) -> u64 {
        self.0
    }
}

impl DvdbndFile {
    pub fn unpadded_len(&self) -> u32 {
        match self.unpadded_len {
            0 => self.len,
            len => len,
        }
    }
}

impl Config for DvdbndConfig {
    const SEPARATORS: &[char] = &['/', '\\'];
}
