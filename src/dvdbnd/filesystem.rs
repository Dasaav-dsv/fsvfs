use std::borrow::Cow;

use crate::filesystem::readonly::{Config, ReadOnlyFilesystem, Rofs};

mod encryption;

#[cfg_attr(feature = "rkyv", derive(rkyv::Archive, rkyv::Serialize))]
#[derive(Debug)]
pub struct BndFs<F>
where
    F: ReadOnlyFilesystem<File = File>,
{
    inner: F,
}

#[derive(Debug)]
pub struct BndConfig;

#[cfg_attr(feature = "rkyv", derive(rkyv::Archive, rkyv::Serialize))]
#[derive(Debug)]
pub struct File {
    src_index: usize,
    src_offset: u64,
    data_len: u32,
    encryption_index: u32,
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
