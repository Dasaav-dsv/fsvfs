use std::{borrow::Cow, num::NonZero};

use fxhash::{FxBuildHasher, FxHashMap};

use crate::{
    cow::CowExt,
    dvdbnd::{
        bhd5::{ByteOrderExt, format::File as Bhd5File},
        dict::Dictionary,
        filesystem::encryption::EncryptionStore,
    },
    filesystem::readonly::{Config, ReadOnlyFilesystem, Rofs},
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

    fn padded_data_len(&self) -> u32;

    fn encryption_index(&self) -> Option<usize>;
}

#[cfg_attr(feature = "rkyv", derive(rkyv::Archive, rkyv::Serialize))]
#[derive(Debug)]
pub struct BndRofs {
    inner: Rofs<'static, File, BndConfig>,
    encryption_store: Box<[u8]>,
}

#[derive(Debug)]
pub struct BndRofsBuilder<'a, 'b, 'c, O: ByteOrderExt> {
    bhds: FxHashMap<&'a str, Bhd5File<'b, O>>,
    dict: Option<&'c Dictionary>,
}

#[derive(Debug)]
struct BndConfig;

#[cfg_attr(feature = "rkyv", derive(rkyv::Archive, rkyv::Serialize))]
#[derive(Clone, Copy, Debug)]
struct File {
    data_index: u32,
    data_len: u32,
    data_offset: u64,
    padded_data_len: u32,
    encryption_index: Option<NonZero<u32>>,
}

impl<'a, 'b, 'c, O: ByteOrderExt> BndRofsBuilder<'a, 'b, 'c, O> {
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
        I: IntoIterator<Item = (&'a str, Bhd5File<'b, O>)>,
    {
        self.bhds.extend(iter);
        self
    }

    pub fn finish(&mut self) -> BndRofs {
        todo!()
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
    fn padded_data_len(&self) -> u32 {
        self.padded_data_len
    }

    #[inline]
    fn encryption_index(&self) -> Option<usize> {
        self.encryption_index.map(|index| index.get() as usize ^ 1)
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
    fn padded_data_len(&self) -> u32 {
        self.padded_data_len.to_native()
    }

    #[inline]
    fn encryption_index(&self) -> Option<usize> {
        self.encryption_index
            .as_ref()
            .map(|index| index.get() as usize ^ 1)
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
