use std::{mem::offset_of, path::Path};

use zerocopy::{BE, I32, Immutable, KnownLayout, LE, TryFromBytes, U32, U64, Unaligned};

use crate::dvdbnd::bhd5::{
    byte_order::{Bom, ByteOrderExt, OneU32},
    consts::Zero,
    magic::Bhd5Magic,
};

mod byte_order;
mod consts;
mod magic;

pub const BHD5_HEADER_LEN: usize = {
    let size = size_of::<Bhd5Header<LE>>();
    assert!(size == size_of::<Bhd5Header<BE>>());
    size
};

#[derive(Clone, Copy, Debug, KnownLayout, Immutable, Unaligned, TryFromBytes)]
#[repr(C, align(1))]
pub struct Bhd5Header<O: ByteOrderExt> {
    /// "BHD5" magic.
    magic: Bhd5Magic,

    /// 0 = big endian, 0xff = little endian.
    byte_order: Bom<O>,

    /// 0 or 1.
    unk05: bool,

    /// Always zero, padding for `unk08`.
    unk06: Zero,

    /// Always zero, padding for `unk08`.
    unk07: Zero,

    /// Always one.
    unk08: OneU32<O>,

    /// Unpadded (e.g. without mandatory RSA block padding) file size.
    file_size: U32<O>,

    /// Number of buckets in the respective header.
    bucket_count: I32<O>,

    /// 32-bit offset of bucket header (non-zero) or padding (always zero) for the DSR format.
    bucket_offset: U32<O>,

    /// 64-bit offset of bucket header (DSR format) or 32-bit length of a "salt" string.
    bucket_offset2_or_salt_len: U64<O>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Bhd5Format {
    DarkSouls,
    DarkSoulsRemastered,
    DarkSouls2,
    DarkSouls3,
    EldenRing,
}

impl<O: ByteOrderExt> Bhd5Header<O> {
    pub const IS_LE: bool = O::IS_LE;
    pub const SALT_OFFSET: u32 = offset_of!(Self, bucket_offset) as u32 + 4;

    pub fn is_dsr_format(&self) -> bool {
        self.bucket_offset == U32::ZERO
    }

    pub fn bucket_count(&self) -> usize {
        let bucket_count = self.bucket_count.get();
        usize::try_from(bucket_count).unwrap_or(0)
    }

    pub fn bucket_offset(&self) -> u64 {
        if self.is_dsr_format() {
            self.bucket_offset2_or_salt_len.get()
        } else {
            self.bucket_offset.get() as u64
        }
    }

    pub fn salt_len(&self) -> Option<usize> {
        if !self.is_dsr_format() {
            let bucket_offset = self.bucket_offset.get();

            let [b0, b1, b2, b3, ..] = self.bucket_offset2_or_salt_len.to_bytes();
            let salt_len = U32::<O>::from_bytes([b0, b1, b2, b3]).get();

            return (Self::SALT_OFFSET <= bucket_offset.saturating_sub(salt_len))
                .then_some(salt_len as usize);
        }

        None
    }
}

pub fn has_bhd_extension<P: AsRef<Path>>(path: P) -> bool {
    path.as_ref()
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("bhd") || ext.eq_ignore_ascii_case("bhd5"))
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
    use zerocopy::{BE, LE, TryFromBytes};

    use crate::dvdbnd::bhd5::{Bhd5Header, has_bhd_extension, strip_bhd_extension};

    #[test]
    fn ds1_bhd_header() {
        test_bhd_header(
            &[
                0x42, 0x48, 0x44, 0x35, 0xFF, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x50, 0x02,
                0x00, 0x00, 0x05, 0x00, 0x00, 0x00, 0x18, 0x00, 0x00, 0x00, 0x06, 0x00, 0x00, 0x00,
                0x40, 0x00, 0x00, 0x00, 0x08, 0x00, 0x00, 0x00, 0xA0, 0x00, 0x00, 0x00,
            ],
            5,
            24,
            None,
        );
    }

    #[test]
    fn ds3_bhd_header() {
        test_bhd_header(
            &[
                0x42, 0x48, 0x44, 0x35, 0xFF, 0x01, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0xA5, 0x9F,
                0x01, 0x00, 0x67, 0x00, 0x00, 0x00, 0x25, 0x00, 0x00, 0x00, 0x09, 0x00, 0x00, 0x00,
                0x46, 0x44, 0x50, 0x5F, 0x70, 0x61, 0x72, 0x74, 0x73,
            ],
            103,
            37,
            Some(9),
        );
    }

    #[test]
    fn er_bhd_header() {
        test_bhd_header(
            &[
                0x42, 0x48, 0x44, 0x35, 0xFF, 0x01, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x5A, 0xB2,
                0x05, 0x00, 0xEF, 0x00, 0x00, 0x00, 0x22, 0x00, 0x00, 0x00, 0x06, 0x00, 0x00, 0x00,
                0x47, 0x52, 0x5F, 0x63, 0x68, 0x72, 0x08, 0x00, 0x00, 0x00, 0x9A, 0x07,
            ],
            239,
            34,
            Some(6),
        );
    }

    #[track_caller]
    fn test_bhd_header(
        bytes: &[u8],
        bucket_count: usize,
        bucket_offset: u64,
        salt_len: Option<usize>,
    ) {
        let _ = Bhd5Header::<BE>::try_ref_from_prefix(bytes).unwrap_err();
        let header = Bhd5Header::<LE>::try_ref_from_prefix(bytes).unwrap().0;

        assert!(!header.is_dsr_format());
        assert_eq!(header.bucket_count(), bucket_count);
        assert_eq!(header.bucket_offset(), bucket_offset);
        assert_eq!(header.salt_len(), salt_len);
    }

    #[test]
    fn good_bhd_extension() {
        assert!(has_bhd_extension("data1.bhd"));
        assert!(has_bhd_extension("data1.bhd5"));
        assert!(has_bhd_extension("data1.BHD"));
        assert!(has_bhd_extension("data1.BHD5"));
    }

    #[test]
    fn bad_bhd_extension() {
        assert!(!has_bhd_extension("data1.bdt"));
        assert!(!has_bhd_extension("data1.bdt5"));
        assert!(!has_bhd_extension("data1.xyzbdt"));
        assert!(!has_bhd_extension("data1.xyzbdt5"));
    }

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
