use std::{mem::offset_of, ops::Deref, ptr::NonNull};

use thiserror::Error;
use zerocopy::{
    BE, FromBytes, I32, Immutable, KnownLayout, LE, TryCastError, TryFromBytes, U32, U64, Unaligned,
};

use crate::dvdbnd::bhd5::{
    byte_order::{Bom, ByteOrderExt, OneU32, ZeroU32},
    consts::Zero,
    magic::Magic,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Format {
    DarkSouls,
    DarkSoulsRemastered,
    DarkSouls2,
    DarkSouls3,
    EldenRing,
}

#[derive(Clone, Debug)]
pub struct File<'a, O: ByteOrderExt> {
    pub format: Format,
    pub buckets: Vec<Buckets<'a, O>>,
    pub encryption: Vec<Option<&'a Encryption<O>>>,
    pub salt: Option<&'a [u8]>,
}

#[derive(Debug, Error)]
pub enum TryRefFileError<O: ByteOrderExt> {
    #[error(transparent)]
    Header(#[from] TryCastError<&'static [()], Header<O>>),

    #[error("malformed bucket file entry")]
    Bucket,

    #[error("malformed file entry encryption data")]
    Encryption,
}

#[derive(Clone, Copy, Debug)]
pub enum Buckets<'a, O: ByteOrderExt> {
    DarkSouls(&'a [FileEntryDs<O>]),
    DarkSouls2(&'a [FileEntryDs2<O>]),
    DarkSouls3(&'a [FileEntryDs3<O>]),
    EldenRing(&'a [FileEntryEr<O>]),
}

pub const BHD5_HEADER_LEN: usize = {
    let size = size_of::<Header<LE>>();
    assert!(size == size_of::<Header<BE>>());
    size
};

#[derive(Clone, Copy, Debug, KnownLayout, Immutable, Unaligned, TryFromBytes)]
#[repr(C, align(1))]
pub struct Header<O: ByteOrderExt> {
    /// "BHD5" magic.
    magic: Magic,

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

#[derive(Clone, Copy, Debug, KnownLayout, Immutable, Unaligned, FromBytes)]
#[repr(C, align(1))]
struct BucketHeader<O: ByteOrderExt> {
    entry_count: I32<O>,
    entry_offset: U32<O>,
}

#[derive(Clone, Copy, Debug, KnownLayout, Immutable, Unaligned, TryFromBytes)]
#[repr(C, align(1))]
struct BucketHeaderDsr<O: ByteOrderExt> {
    entry_count: I32<O>,
    unk04: OneU32<O>,
    entry_offset: U64<O>,
}

#[derive(Clone, Copy, Debug, KnownLayout, Immutable, Unaligned, FromBytes)]
#[repr(C, align(1))]
pub struct FileEntryDs<O: ByteOrderExt> {
    pub path_hash: U32<O>,
    pub file_size: I32<O>,
    pub file_offset: U64<O>,
}

#[derive(Clone, Copy, Debug, KnownLayout, Immutable, Unaligned, FromBytes)]
#[repr(C, align(1))]
pub struct FileEntryDs2<O: ByteOrderExt> {
    pub path_hash: U32<O>,
    pub file_size: I32<O>,
    pub file_offset: U64<O>,
    pub file_hash_offset: U64<O>,
    pub encryption_offset: U64<O>,
}

#[derive(Clone, Copy, Debug, KnownLayout, Immutable, Unaligned, TryFromBytes)]
#[repr(C, align(1))]
pub struct FileEntryDs3<O: ByteOrderExt> {
    pub inner: FileEntryDs2<O>,
    pub unpadded_file_size: I32<O>,
    unk24: ZeroU32<O>,
}

#[derive(Clone, Copy, Debug, KnownLayout, Immutable, Unaligned, FromBytes)]
#[repr(C, align(1))]
pub struct FileEntryEr<O: ByteOrderExt> {
    pub path_hash: U64<O>,
    pub file_size: I32<O>,
    pub unpadded_file_size: I32<O>,
    pub file_offset: U64<O>,
    pub file_hash_offset: U64<O>,
    pub encryption_offset: U64<O>,
}

#[derive(Debug, KnownLayout, Immutable, Unaligned, FromBytes)]
#[repr(C, align(1))]
pub struct Encryption<O: ByteOrderExt> {
    pub key: [u8; 16],
    pub range_count: I32<O>,
    pub ranges: [EncryptionRange<O>],
}

#[derive(Clone, Copy, Debug, KnownLayout, Immutable, Unaligned, FromBytes)]
#[repr(C, align(1))]
pub struct EncryptionRange<O: ByteOrderExt> {
    pub start_offset: U64<O>,
    pub end_offset: U64<O>,
}

impl<'a, O: ByteOrderExt> File<'a, O> {
    pub fn try_ref_from_bytes(bytes: &'a [u8]) -> Result<Self, TryRefFileError<O>> {
        let header = Self::try_ref_header(bytes)?;

        let mut file = match header.salt_len() {
            Some(salt_len) => Self::try_ref_ds2_ds3_er(bytes, header, salt_len)?,
            None => {
                if header.is_dsr_format() {
                    Self::try_ref_dsr(bytes, header)?
                } else {
                    Self::try_ref_ds1(bytes, header)?
                }
            }
        };

        file.try_ref_encryption(bytes)?;

        Ok(file)
    }

    fn try_ref_encryption(&mut self, bytes: &'a [u8]) -> Result<(), TryRefFileError<O>> {
        let total_len = self.buckets.iter().map(Buckets::len).sum();

        self.encryption = vec![None; total_len];

        if matches!(self.format, Format::DarkSouls | Format::DarkSoulsRemastered) {
            return Ok(());
        }

        macro_rules! try_ref_encryption {
            ($self:ident, $inner:ident, $start_index:ident) => {
                for (entry, encryption_out) in
                    $inner.iter().zip(&mut $self.encryption[$start_index..])
                {
                    if entry.encryption_offset == U64::ZERO {
                        continue;
                    }

                    let offset = usize::try_from(entry.encryption_offset.get())
                        .map_err(|_| TryRefFileError::Encryption)?;

                    let bytes = bytes.get(offset..).ok_or(TryRefFileError::Encryption)?;
                    let (encryption, _) = Encryption::<O>::ref_from_prefix_with_elems(bytes, 0)
                        .map_err(|_| TryRefFileError::Encryption)?;

                    let n_ranges = usize::try_from(encryption.range_count.get())
                        .map_err(|_| TryRefFileError::Encryption)?;

                    let (encryption, _) =
                        Encryption::<O>::ref_from_prefix_with_elems(bytes, n_ranges)
                            .map_err(|_| TryRefFileError::Encryption)?;

                    *encryption_out = Some(encryption);
                }
            };
        }

        for (buckets, start_index) in self.buckets.iter().scan(0, |sum, buckets| {
            let index = *sum;
            *sum += buckets.len();
            Some((buckets, index))
        }) {
            match buckets {
                Buckets::DarkSouls2(entry) => try_ref_encryption!(self, entry, start_index),
                Buckets::DarkSouls3(entry) => try_ref_encryption!(self, entry, start_index),
                Buckets::EldenRing(entry) => try_ref_encryption!(self, entry, start_index),
                Buckets::DarkSouls(_) => unreachable!("unexpected ds1 format"),
            }
        }

        Ok(())
    }

    fn try_ref_header(
        bytes: &'a [u8],
    ) -> Result<&'a Header<O>, TryCastError<&'static [()], Header<O>>> {
        match Header::<O>::try_ref_from_prefix(bytes) {
            Ok((header, _)) => Ok(header),
            Err(e) => {
                // SAFETY: ZST slice from dangling pointer.
                Err(e.map_src(|src| unsafe {
                    NonNull::slice_from_raw_parts(NonNull::<()>::dangling(), src.len()).as_ref()
                }))
            }
        }
    }

    fn try_ref_ds1(bytes: &'a [u8], header: &Header<O>) -> Result<Self, TryRefFileError<O>> {
        let bucket_header = try_ref_slice_helper::<BucketHeader<O>>(
            bytes,
            header.bucket_count(),
            header.bucket_offset(),
        )
        .ok_or(TryRefFileError::Bucket)?;

        let buckets = bucket_header
            .iter()
            .map(|bucket| {
                try_ref_slice_helper(bytes, bucket.entry_count.get(), bucket.entry_offset.get())
                    .map(Buckets::DarkSouls)
            })
            .collect::<Option<Vec<_>>>()
            .ok_or(TryRefFileError::Bucket)?;

        Ok(Self {
            format: Format::DarkSouls,
            buckets,
            encryption: vec![],
            salt: None,
        })
    }

    fn try_ref_dsr(bytes: &'a [u8], header: &Header<O>) -> Result<Self, TryRefFileError<O>> {
        let bucket_header = try_ref_slice_helper::<BucketHeaderDsr<O>>(
            bytes,
            header.bucket_count(),
            header.bucket_offset(),
        )
        .ok_or(TryRefFileError::Bucket)?;

        let buckets = bucket_header
            .iter()
            .map(|bucket| {
                try_ref_slice_helper(bytes, bucket.entry_count.get(), bucket.entry_offset.get())
                    .map(Buckets::DarkSouls)
            })
            .collect::<Option<Vec<_>>>()
            .ok_or(TryRefFileError::Bucket)?;

        Ok(Self {
            format: Format::DarkSoulsRemastered,
            buckets,
            encryption: vec![],
            salt: None,
        })
    }

    fn try_ref_ds2_ds3_er(
        bytes: &'a [u8],
        header: &Header<O>,
        salt_len: usize,
    ) -> Result<Self, TryRefFileError<O>> {
        let salt_start = Header::<O>::SALT_OFFSET;
        let salt = salt_start
            .checked_add(salt_len)
            .and_then(|end| bytes.get(salt_start..end));

        let bucket_header = try_ref_slice_helper::<BucketHeader<O>>(
            bytes,
            header.bucket_count(),
            header.bucket_offset(),
        )
        .ok_or(TryRefFileError::Bucket)?;

        let mut has_offset_0 = false;
        let buckets_ds3 = bucket_header
            .iter()
            .map(|bucket| {
                try_ref_slice_helper(bytes, bucket.entry_count.get(), bucket.entry_offset.get())
                    .filter(|entries| {
                        entries
                            .iter()
                            .all(|entry| FileEntryDs3::is_valid(entry, header, &mut has_offset_0))
                    })
                    .map(Buckets::DarkSouls3)
            })
            .collect::<Option<Vec<_>>>();

        if let Some(buckets) = buckets_ds3 {
            return Ok(Self {
                format: Format::DarkSouls3,
                buckets,
                encryption: vec![],
                salt,
            });
        }

        let mut has_offset_0 = false;
        let buckets_ds2 = bucket_header
            .iter()
            .map(|bucket| {
                try_ref_slice_helper(bytes, bucket.entry_count.get(), bucket.entry_offset.get())
                    .filter(|entries| {
                        entries
                            .iter()
                            .all(|entry| FileEntryDs2::is_valid(entry, header, &mut has_offset_0))
                    })
                    .map(Buckets::DarkSouls2)
            })
            .collect::<Option<Vec<_>>>();

        if let Some(buckets) = buckets_ds2 {
            return Ok(Self {
                format: Format::DarkSouls2,
                buckets,
                encryption: vec![],
                salt,
            });
        }

        let buckets = bucket_header
            .iter()
            .map(|bucket| {
                try_ref_slice_helper(bytes, bucket.entry_count.get(), bucket.entry_offset.get())
                    .map(Buckets::EldenRing)
            })
            .collect::<Option<Vec<_>>>()
            .ok_or(TryRefFileError::Bucket)?;

        Ok(Self {
            format: Format::EldenRing,
            buckets,
            encryption: vec![],
            salt,
        })
    }
}

fn try_ref_slice_helper<'a, B: Immutable + TryFromBytes>(
    bytes: &'a [u8],
    count: impl TryInto<usize>,
    offset: impl TryInto<usize>,
) -> Option<&'a [B]> {
    let count = count.try_into().ok()?;
    let offset = offset.try_into().ok()?;

    let bytes = bytes.get(offset..)?;
    let (slice, _) = <[B]>::try_ref_from_prefix_with_elems(bytes, count).ok()?;

    Some(slice)
}

impl<O: ByteOrderExt> Header<O> {
    pub const IS_LE: bool = O::IS_LE;
    pub const SALT_OFFSET: usize = offset_of!(Self, bucket_offset) + 4;

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
        if self.is_dsr_format() {
            return None;
        }

        let bucket_offset = self.bucket_offset.get();

        let [b0, b1, b2, b3, ..] = self.bucket_offset2_or_salt_len.to_bytes();
        let salt_len = U32::<O>::from_bytes([b0, b1, b2, b3]).get();

        ((Self::SALT_OFFSET as u32).checked_add(salt_len)? <= bucket_offset)
            .then_some(salt_len as usize)
    }
}

impl<O: ByteOrderExt> Buckets<'_, O> {
    fn len(&self) -> usize {
        match self {
            Self::DarkSouls(inner) => inner.len(),
            Self::DarkSouls2(inner) => inner.len(),
            Self::DarkSouls3(inner) => inner.len(),
            Self::EldenRing(inner) => inner.len(),
        }
    }
}

impl<O: ByteOrderExt> FileEntryDs2<O> {
    fn is_valid(&self, header: &Header<O>, has_offset_0: &mut bool) -> bool {
        let is_offset_0 = self.file_offset == U64::ZERO;

        if is_offset_0 && *has_offset_0 {
            return false;
        }

        *has_offset_0 |= is_offset_0;

        let header_size = header.file_size.get() as u64;

        self.file_size >= 0
            && self.encryption_offset < header_size
            && self.file_hash_offset < header_size
    }
}

impl<O: ByteOrderExt> FileEntryDs3<O> {
    fn is_valid(&self, header: &Header<O>, has_offset_0: &mut bool) -> bool {
        if !self.inner.is_valid(header, has_offset_0) {
            return false;
        }

        let unpadded_file_size = self.unpadded_file_size.get();

        unpadded_file_size >= 0 && self.file_size >= unpadded_file_size
    }
}

impl<O: ByteOrderExt> Deref for FileEntryDs3<O> {
    type Target = FileEntryDs2<O>;

    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

#[cfg(test)]
mod tests {
    use std::{fs::File, io::Read};

    use zerocopy::{BE, LE, TryFromBytes};

    use crate::{
        crypto::rsa::RsaDecryptor,
        dvdbnd::bhd5::format::{File as Bhd5File, Format, Header},
        tests::SteamAppId,
    };

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
        let _ = Header::<BE>::try_ref_from_prefix(bytes).unwrap_err();
        let header = Header::<LE>::try_ref_from_prefix(bytes).unwrap().0;

        assert!(!header.is_dsr_format());
        assert_eq!(header.bucket_count(), bucket_count);
        assert_eq!(header.bucket_offset(), bucket_offset);
        assert_eq!(header.salt_len(), salt_len);
    }

    #[test]
    #[ignore]
    fn ds1_bhd_format() {
        expect_bhd_format(SteamAppId::DarkSouls, Format::DarkSouls);
    }

    #[test]
    #[ignore]
    fn ds2_bhd_format() {
        expect_bhd_format(SteamAppId::DarkSouls2, Format::DarkSouls2);
    }

    #[test]
    #[ignore]
    fn ds2s_bhd_format() {
        expect_bhd_format(SteamAppId::DarkSouls2SotFS, Format::DarkSouls2);
    }

    #[test]
    #[ignore]
    fn ds3_bhd_format() {
        expect_bhd_format(SteamAppId::DarkSouls3, Format::DarkSouls3);
    }

    #[test]
    #[ignore]
    fn sekiro_bhd_format() {
        expect_bhd_format(SteamAppId::Sekiro, Format::DarkSouls3);
    }

    #[test]
    #[ignore]
    fn sekiro_ost_bhd_format() {
        expect_bhd_format(SteamAppId::SekiroSoundtrack, Format::DarkSouls3);
    }

    #[test]
    #[ignore]
    fn er_bhd_format() {
        expect_bhd_format(SteamAppId::EldenRing, Format::EldenRing);
    }

    #[test]
    #[ignore]
    fn ac6_bhd_format() {
        expect_bhd_format(SteamAppId::ArmoredCore6, Format::EldenRing);
    }

    #[test]
    #[ignore]
    fn nr_bhd_format() {
        expect_bhd_format(SteamAppId::Nightreign, Format::EldenRing);
    }

    #[track_caller]
    fn expect_bhd_format(game: SteamAppId, format: Format) {
        let keys = game.bhd_keys().unwrap();
        let files = keys
            .by_path
            .iter()
            .map(|(path, key)| {
                let mut bytes = vec![];
                let mut file = File::open(path).unwrap();

                match key {
                    Some(key) => RsaDecryptor::new(key, file)
                        .read_to_end(&mut bytes)
                        .unwrap(),
                    None => file.read_to_end(&mut bytes).unwrap(),
                };

                (*path, bytes)
            })
            .collect::<Vec<_>>();

        for (path, bytes) in files {
            let file = Bhd5File::<LE>::try_ref_from_bytes(&bytes).unwrap();
            assert_eq!(file.format, format, "{:?}", path.as_ref());
        }
    }
}
