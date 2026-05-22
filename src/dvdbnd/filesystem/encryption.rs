use aes::cipher::{
    BlockCipherDecrypt, KeyInit,
    array::{AsArrayMut, AsArrayRef, AssocArraySize},
};
use thiserror::Error;
use zerocopy::{FromBytes, Immutable, KnownLayout, TryFromBytes, Unaligned};

use crate::unaligned::{U16, U24, U32};

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

#[cfg_attr(
    feature = "rkyv",
    derive(rkyv::Archive, rkyv::Serialize, rkyv::Portable)
)]
#[derive(Clone, Copy, Debug, KnownLayout, Immutable, Unaligned, TryFromBytes)]
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
#[derive(Clone, Copy, Debug, KnownLayout, Immutable, Unaligned, FromBytes)]
#[repr(transparent)]
struct Aes128(Aes<16>);

#[cfg_attr(
    feature = "rkyv",
    derive(rkyv::Archive, rkyv::Serialize, rkyv::Portable)
)]
#[derive(Clone, Copy, Debug, KnownLayout, Immutable, Unaligned, FromBytes)]
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
#[derive(Clone, Copy, Debug, KnownLayout, Immutable, Unaligned, FromBytes)]
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
