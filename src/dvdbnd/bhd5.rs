use color_eyre::eyre;
use xxhash_rust::{const_xxh3, xxh3::xxh3_128_with_seed};
use zerocopy::{BE, LE};

use crate::dvdbnd::bhd5::format::File;

mod byte_order;
mod consts;
pub mod format;
mod magic;

pub use byte_order::ByteOrderExt;

#[derive(Debug)]
pub struct Bhd5File<'a> {
    pub kind: Bhd5FileKind<'a>,
    pub hash: u128,
}

#[derive(Debug)]
pub enum Bhd5FileKind<'a> {
    LE(File<'a, LE>),
    BE(File<'a, BE>),
}

impl<'a> Bhd5File<'a> {
    pub fn try_ref_from_bytes(bytes: &'a [u8]) -> eyre::Result<Self> {
        const SEED: u64 = const_xxh3::xxh3_64(env!("CARGO_PKG_VERSION").as_bytes());

        match File::<LE>::try_ref_from_bytes(bytes) {
            Ok(le) => Ok(Self {
                kind: Bhd5FileKind::LE(le),
                hash: xxh3_128_with_seed(bytes, SEED),
            }),
            Err(err_le) => match File::<BE>::try_ref_from_bytes(bytes) {
                Ok(be) => Ok(Self {
                    kind: Bhd5FileKind::BE(be),
                    hash: xxh3_128_with_seed(bytes, SEED),
                }),
                Err(err_be) => Err(eyre::eyre!(
                    "LE ref error: {err_le}; BE ref error: {err_be}"
                )),
            },
        }
    }
}

pub fn strip_bhd_extension(name: &str) -> Option<&str> {
    let name = name.strip_suffix("5").unwrap_or(name);

    let (name, suffix) = name
        .as_bytes()
        .split_at_checked(name.len().wrapping_sub(4))?;

    suffix
        .eq_ignore_ascii_case(b".bhd")
        .then(|| // SAFETY: valid UTF-8 slice before ".bhd"
             unsafe { str::from_utf8_unchecked(name) })
}

#[cfg(test)]
mod tests {
    use crate::dvdbnd::bhd5::strip_bhd_extension;

    #[test]
    fn strip_good_bhd_extension() {
        assert_eq!(strip_bhd_extension("data1.bhd"), Some("data1"));
        assert_eq!(strip_bhd_extension("data1.bhd5"), Some("data1"));
        assert_eq!(strip_bhd_extension("DATA1.BHD"), Some("DATA1"));
        assert_eq!(strip_bhd_extension("DATA1.BHD5"), Some("DATA1"));
    }

    #[test]
    fn strip_bad_bhd_extension() {
        assert_eq!(strip_bhd_extension("data1.bdt"), None);
        assert_eq!(strip_bhd_extension("data1.bdt5"), None);
        assert_eq!(strip_bhd_extension("data1.xyzbdt"), None);
        assert_eq!(strip_bhd_extension("data1.xyzbdt5"), None);
    }
}
