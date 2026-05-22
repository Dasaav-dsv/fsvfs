use std::{borrow::Cow, num::NonZero};

use crate::{
    dvdbnd::{
        bhd5::{ByteOrderExt, format::File as Bhd5File},
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

impl BndRofs {
    pub fn new<O: ByteOrderExt>(headers: &[&Bhd5File<O>], dict: ()) -> Self {
        todo!()
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
        if component.as_bytes().chunks(8).any(|chunk| {
            chunk
                .iter()
                .fold(false, |is, byte| is | byte.is_ascii_uppercase())
        }) {
            Cow::Owned(component.to_ascii_lowercase())
        } else {
            Cow::Borrowed(component)
        }
    }
}
