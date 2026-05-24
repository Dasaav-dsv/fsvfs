use std::num::NonZero;

use aes::cipher::{
    BlockCipherDecrypt, KeyInit,
    array::{AsArrayMut, AsArrayRef, AssocArraySize},
};
use thiserror::Error;
use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout, TryFromBytes, Unaligned};

use crate::{
    dvdbnd::bhd5::{
        ByteOrderExt,
        format::{Encryption, FileEntry as Bhd5Entry},
    },
    unaligned::{U16, U24, U32},
};

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("too many encryption entries")]
    TooManyEntries,

    #[error("too many encrypted ranges")]
    TooManyRanges,

    #[error("the encrypted range ({0}..{1}) is invalid")]
    BadRange(u64, u64),

    #[error("file (at {2}; size {3}) does not contain the encrypted range ({0}..{1})")]
    OobRange(u64, u64, u64, u32),
}

#[derive(Debug, Error)]
pub enum DecryptError {
    #[error("the specified index ({0}) is invalid (this indicates a bug in fsvfs)")]
    Index(usize),

    #[error("block (at file offset {0}) is out of bounds")]
    Block(u32),
}

pub trait EncryptionStore {
    fn decrypt(&self, index: usize, bytes: &mut [u8]) -> Result<(), DecryptError>;
}

impl<T: AsRef<[u8]>> EncryptionStore for T {
    fn decrypt(&self, index: usize, bytes: &mut [u8]) -> Result<(), DecryptError> {
        let store = self
            .as_ref()
            .get(index..)
            .ok_or(DecryptError::Index(index))?;

        let (header, data) =
            Header::try_ref_from_prefix(store).map_err(|_| DecryptError::Index(index))?;

        header.decrypt(index, bytes, data)
    }
}

pub fn store_encryption<O, E>(
    entry: &E,
    encryption: &Encryption<O>,
    out: &mut Vec<u8>,
) -> Result<usize, StoreError>
where
    O: ByteOrderExt,
    E: Bhd5Entry<O>,
{
    let index = (NonZero::new(out.len() ^ usize::MAX).unwrap())
        .try_into()
        .map_err(|_| StoreError::TooManyEntries)?;

    const BLOCK_SIZE: u32 = 16;
    const MAX_BLOCK_COUNT: u32 = u16::MAX as u32 + 1;

    let file_offset = entry.file_offset();
    let file_size = entry.file_size() as u64;

    let mut ranges = Vec::with_capacity(encryption.ranges.len());

    for range in &encryption.ranges {
        let (start, end) = (range.start_offset.get(), range.end_offset.get());
        let len = start.wrapping_sub(end);

        if start > end || !len.is_multiple_of(BLOCK_SIZE as u64) {
            return Err(StoreError::BadRange(start, end));
        }

        let start_offset = start.wrapping_sub(file_offset);
        if start_offset > file_size.saturating_sub(len) {
            return Err(StoreError::OobRange(
                start,
                end,
                file_offset,
                file_size as u32,
            ));
        }

        let block_count = len as u32 / BLOCK_SIZE;
        let mut start_offset = start_offset as u32;

        for _ in 0..block_count / MAX_BLOCK_COUNT {
            ranges.push(Range {
                start_offset: U32::new(start_offset),
                block_count: U16::MAX,
            });

            start_offset += MAX_BLOCK_COUNT * BLOCK_SIZE;
        }

        ranges.push(Range {
            start_offset: U32::new(start_offset),
            block_count: U16::new((block_count % MAX_BLOCK_COUNT) as u16),
        });
    }

    let key = encryption.key;
    let range_count = u32::try_from(ranges.len())
        .and_then(U24::try_from)
        .map_err(|_| StoreError::TooManyRanges)?;

    let header = Header::Aes128EcbNone(Aes128(Aes {
        key,
        iv: (),
        range_count,
    }));

    out.extend_from_slice(header.as_bytes());
    out.extend_from_slice(ranges.as_bytes());

    Ok(index)
}

#[cfg_attr(
    feature = "rkyv",
    derive(rkyv::Archive, rkyv::Serialize, rkyv::Portable)
)]
#[derive(Clone, Copy, Debug, KnownLayout, Immutable, Unaligned, IntoBytes, TryFromBytes)]
#[repr(u8)]
enum Header {
    Aes128EcbNone(Aes128),
}

#[derive(Debug, Error)]
enum AesError {
    #[error("malformed range")]
    Range,

    #[error("block (at file offset {0}) is too short or out of bounds")]
    Block(u32),
}

#[cfg_attr(
    feature = "rkyv",
    derive(rkyv::Archive, rkyv::Serialize, rkyv::Portable)
)]
#[derive(Clone, Copy, Debug, KnownLayout, Immutable, Unaligned, IntoBytes, FromBytes)]
#[repr(transparent)]
struct Aes128(Aes<16>);

#[cfg_attr(
    feature = "rkyv",
    derive(rkyv::Archive, rkyv::Serialize, rkyv::Portable)
)]
#[derive(Clone, Copy, Debug, KnownLayout, Immutable, Unaligned, IntoBytes, FromBytes)]
#[repr(C, align(1))]
struct Aes<const N: usize, Iv = ()> {
    key: [u8; N],
    iv: Iv,
    range_count: U24,
}

#[cfg_attr(
    feature = "rkyv",
    derive(rkyv::Archive, rkyv::Serialize, rkyv::Portable)
)]
#[derive(Clone, Copy, Debug, KnownLayout, Immutable, Unaligned, IntoBytes, FromBytes)]
#[repr(C, align(1))]
struct Range {
    start_offset: U32,
    block_count: U16,
}

impl Header {
    fn decrypt(&self, index: usize, bytes: &mut [u8], data: &[u8]) -> Result<(), DecryptError> {
        match self {
            Self::Aes128EcbNone(Aes128(aes)) => {
                let key = aes::Aes128::new(aes.key.as_array_ref());

                aes.decrypt_blocks(&key, bytes, data).map_err(|e| match e {
                    AesError::Range => DecryptError::Index(index),
                    AesError::Block(offset) => DecryptError::Block(offset),
                })?;
            }
        }

        Ok(())
    }
}

impl<const N: usize> Aes<N> {
    fn decrypt_blocks<C: BlockCipherDecrypt>(
        &self,
        cipher: &C,
        bytes: &mut [u8],
        data: &[u8],
    ) -> Result<(), AesError>
    where
        [u8; N]: AssocArraySize<Size = C::BlockSize> + AsArrayMut<u8>,
    {
        let range_count = self.range_count.get() as usize;
        let ranges =
            <[Range]>::ref_from_bytes_with_elems(data, range_count).map_err(|_| AesError::Range)?;

        for range in ranges {
            let start = range.start_offset.get() as usize;
            let end = start + range.block_count.get() as usize * N;

            let len = bytes.len();
            let range = bytes.get_mut(start..end).ok_or_else(|| {
                let last_offset = len & N.wrapping_neg();
                AesError::Block(last_offset as u32)
            })?;

            for block in range
                .chunks_exact_mut(N)
                .filter_map(<[_]>::as_mut_array::<N>)
            {
                cipher.decrypt_block(block.as_array_mut());
            }
        }

        Ok(())
    }
}
