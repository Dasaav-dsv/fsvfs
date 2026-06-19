use std::num::NonZero;

use aes::cipher::{
    BlockCipherDecrypt, KeyInit,
    array::{AsArrayMut, AsArrayRef},
};
use async_trait::async_trait;
use futures_util::TryFutureExt;
use rkyv::{Archive, Deserialize, Portable, Serialize};
use thiserror::Error;
use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout, TryFromBytes, Unaligned};

use crate::{
    dvdbnd::{
        bhd5::{
            ByteOrderExt,
            format::{Encryption, FileEntry as Bhd5Entry},
        },
        filesystem::aligned::AlignedDynBufferRef,
    },
    unaligned::{U24, U32},
};

pub const BLOCK_SIZE: usize = 16;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("too many encrypted ranges")]
    TooManyRanges,

    #[error("the encrypted range ({0}..{1}) is invalid")]
    BadRange(u64, u64),

    #[error("file (at {2}; size {3}) does not contain the encrypted range ({0}..{1})")]
    OobRange(u64, u64, u64, u32),
}

#[derive(Debug, Error)]
pub enum DecryptError {
    #[error("the encryption id ({0:?}) is invalid")]
    Id(EncryptionId),
}

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Archive, Serialize, Deserialize,
)]
#[repr(transparent)]
pub struct EncryptionId(NonZero<u32>);

#[derive(Debug)]
pub struct Ciphertext<'a> {
    body: &'a mut [u8],
    head: [u8; BLOCK_SIZE],
    tail: [u8; BLOCK_SIZE],
    file_offset: u32,
}

#[derive(Default)]
pub struct CiphertextBuffer {
    inner: [Option<AlignedDynBufferRef>; 3],
    pos: Pos,
    is_pushed: bool,
}

#[derive(Clone, Copy, Default, Debug)]
enum Pos {
    _0 = 0,
    #[default]
    _1 = 1,
    _2 = 2,
}

#[async_trait(?Send)]
pub trait EncryptionStore {
    async fn decrypt<'a>(
        &'a self,
        id: EncryptionId,
        ciphertext: Ciphertext<'a>,
    ) -> Result<(), DecryptError>;
}

impl EncryptionId {
    fn try_from_index(index: usize) -> Option<Self> {
        let non_zero = NonZero::<usize>::new(index ^ u32::MAX as usize)?;
        non_zero.try_into().ok().map(Self)
    }

    fn into_index(self) -> usize {
        (self.0.get() ^ u32::MAX) as usize
    }
}

impl<'a> Ciphertext<'a> {
    pub fn from_buffer(buf: &'a mut CiphertextBuffer) -> Self {
        let prev = buf.prev();
        let next = buf.next();

        let left = BLOCK_SIZE.saturating_sub(prev.len());
        let right = prev.len().saturating_sub(BLOCK_SIZE);

        let mut head = [0; BLOCK_SIZE];
        head[left..].copy_from_slice(&prev[right..]);

        let len = next.len().min(BLOCK_SIZE);

        let mut tail = [0; BLOCK_SIZE];
        tail[..len].copy_from_slice(&next[..len]);

        let (body, file_offset) = buf.curr();

        let file_offset =
            u32::try_from(*file_offset).expect("current file offset must not be negative");

        Self {
            body,
            head,
            tail,
            file_offset,
        }
    }
}

impl CiphertextBuffer {
    pub fn push(&mut self, buf: AlignedDynBufferRef) {
        self.do_push(Some(buf));
    }

    pub fn finish(&mut self) {
        self.do_push(None);
    }

    pub fn curr(&mut self) -> &mut AlignedDynBufferRef {
        if self.has_predecessor() {
            self.inner[self.pos.prev() as usize].as_mut().unwrap()
        } else {
            self.inner[self.pos as usize]
                .as_mut()
                .expect("must not be empty")
        }
    }

    pub fn is_empty(&self) -> bool {
        !self.is_pushed
    }

    fn do_push(&mut self, buf: Option<AlignedDynBufferRef>) {
        self.pos = self.pos.next();
        self.inner[self.pos as usize] = buf;
        self.is_pushed |= true;
    }

    fn prev(&self) -> &[u8] {
        if !self.has_predecessor() {
            return &[];
        }

        self.inner[self.pos.prev().prev() as usize]
            .as_ref()
            .map(|buf| &**buf.0)
            .unwrap_or(&[])
    }

    fn next(&self) -> &[u8] {
        if !self.has_predecessor() {
            return &[];
        }

        self.inner[self.pos as usize]
            .as_ref()
            .map(|buf| &**buf.0)
            .unwrap_or(&[])
    }

    fn has_predecessor(&self) -> bool {
        self.inner[self.pos.prev() as usize].is_some()
    }
}

impl Pos {
    fn next(self) -> Self {
        match self {
            Self::_0 => Self::_1,
            Self::_1 => Self::_2,
            Self::_2 => Self::_0,
        }
    }

    fn prev(self) -> Self {
        match self {
            Self::_0 => Self::_2,
            Self::_1 => Self::_0,
            Self::_2 => Self::_1,
        }
    }
}

#[async_trait(?Send)]
impl<S: AsRef<[u8]>> EncryptionStore for S {
    async fn decrypt<'a>(
        &'a self,
        id: EncryptionId,
        ciphertext: Ciphertext<'a>,
    ) -> Result<(), DecryptError> {
        let index = id.into_index();

        let store = self.as_ref().get(index..).ok_or(DecryptError::Id(id))?;

        let (header, data) =
            Header::try_ref_from_prefix(store).map_err(|_| DecryptError::Id(id))?;

        header.decrypt(id, ciphertext, data).await
    }
}

pub fn store_encryption<O, E>(
    entry: &E,
    encryption: &Encryption<O>,
    out: &mut Vec<u8>,
) -> Result<EncryptionId, StoreError>
where
    O: ByteOrderExt,
    E: Bhd5Entry<O>,
{
    let file_offset = entry.file_offset();
    let file_size = entry.file_size() as u64;

    let mut ranges = Vec::with_capacity(encryption.ranges.len());

    for range in &encryption.ranges {
        let (start, end) = (range.start_offset.get(), range.end_offset.get());
        let len = end.wrapping_sub(start);

        if len == 0 {
            continue;
        }

        if start > end || !len.is_multiple_of(BLOCK_SIZE as u64) {
            return Err(StoreError::BadRange(start, end));
        }

        if start > file_size.saturating_sub(len) {
            return Err(StoreError::OobRange(
                start,
                end,
                file_offset,
                file_size as u32,
            ));
        }

        let block_count = len as u32 / BLOCK_SIZE as u32;

        ranges.push(Range {
            start_offset: U32::new(start as u32),
            block_count: U32::new(block_count),
        });
    }

    ranges.sort_by_key(|range| range.end_offset());

    let key = encryption.key;
    let range_count = u32::try_from(ranges.len())
        .and_then(U24::try_from)
        .map_err(|_| StoreError::TooManyRanges)?;

    let header = Header::Aes128EcbNone(Aes128 { key, range_count });
    let index = EncryptionId::try_from_index(out.len()).ok_or(StoreError::TooManyRanges)?;

    out.extend_from_slice(header.as_bytes());
    out.extend_from_slice(ranges.as_bytes());

    Ok(index)
}

#[derive(
    Clone,
    Debug,
    KnownLayout,
    Immutable,
    Unaligned,
    IntoBytes,
    TryFromBytes,
    Archive,
    Serialize,
    Portable,
)]
#[repr(u8)]
enum Header {
    Aes128EcbNone(Aes128),
}

#[derive(Debug, Error)]
enum AesError {
    #[error("malformed range")]
    Range,
}

#[derive(
    Clone,
    Debug,
    KnownLayout,
    Immutable,
    Unaligned,
    IntoBytes,
    FromBytes,
    Archive,
    Serialize,
    Portable,
)]
#[repr(C, align(1))]
struct Aes128 {
    key: [u8; BLOCK_SIZE],
    range_count: U24,
}

#[derive(
    Clone,
    Copy,
    Debug,
    KnownLayout,
    Immutable,
    Unaligned,
    IntoBytes,
    FromBytes,
    Archive,
    Serialize,
    Portable,
)]
#[repr(C, align(1))]
struct Range {
    start_offset: U32,
    block_count: U32,
}

impl Range {
    fn block_count(&self) -> u32 {
        self.block_count.get()
    }

    fn start_offset(&self) -> u32 {
        self.start_offset.get()
    }

    fn end_offset(&self) -> u32 {
        self.start_offset() + self.block_count() * BLOCK_SIZE as u32
    }
}

impl Header {
    fn decrypt(
        &self,
        id: EncryptionId,
        ciphertext: Ciphertext<'_>,
        data: &[u8],
    ) -> impl Future<Output = Result<(), DecryptError>> {
        match self {
            Self::Aes128EcbNone(aes) => {
                let key = aes::Aes128::new(aes.key.as_array_ref());

                aes.decrypt(key, ciphertext, data)
                    .map_err(move |e| match e {
                        AesError::Range => DecryptError::Id(id),
                    })
            }
        }
    }
}

impl Aes128 {
    async fn decrypt(
        &self,
        cipher: aes::Aes128,
        ciphertext: Ciphertext<'_>,
        ranges: &[u8],
    ) -> Result<(), AesError> {
        let range_count = self.range_count.get() as usize;
        let (ranges, _) = <[Range]>::ref_from_prefix_with_elems(ranges, range_count)
            .map_err(|_| AesError::Range)?;

        let file_offset = ciphertext.file_offset;
        let start_index = ranges.partition_point(|range| file_offset >= range.end_offset());

        let head = ciphertext.head;
        let tail = ciphertext.tail;

        let bytes = ciphertext.body;
        let len = bytes.len();

        for range in &ranges[start_index..] {
            let mut start_offset = range.start_offset.get();

            if start_offset >= file_offset + len as u32 {
                break;
            }

            if let Some(delta) = file_offset.checked_sub(start_offset) {
                start_offset += delta & (BLOCK_SIZE as u32).wrapping_neg();
            }

            let start = if start_offset < file_offset {
                let left = (file_offset - start_offset) as usize % BLOCK_SIZE;
                let left_len = BLOCK_SIZE - left;

                let mid_len = left_len.min(len);

                let right = left + mid_len;
                let right_len = BLOCK_SIZE - right;

                let mut block = [0; BLOCK_SIZE];

                block[..left].copy_from_slice(&head[left_len..]);
                block[left..right].copy_from_slice(&bytes[..mid_len]);
                block[right..].copy_from_slice(&tail[..right_len]);

                cipher.decrypt_block(block.as_array_mut());
                bytes[..mid_len].copy_from_slice(&block[left..right]);

                file_offset as usize + left_len
            } else {
                (start_offset - file_offset) as usize
            };

            let end = Ord::min(len, (range.end_offset() - file_offset) as usize);

            if start >= end {
                continue;
            }

            let range = &mut bytes[start..end];
            let (blocks, rest) = range.as_chunks_mut::<BLOCK_SIZE>();

            async {
                for block in blocks {
                    cipher.decrypt_block(block.as_array_mut());
                }
            }
            .await;

            if !rest.is_empty() {
                let mid = rest.len();
                let right = BLOCK_SIZE - mid;

                let mut block = [0; BLOCK_SIZE];

                block[..mid].copy_from_slice(rest);
                block[mid..].copy_from_slice(&tail[..right]);

                cipher.decrypt_block(block.as_array_mut());
                rest.copy_from_slice(&block[..mid]);
            }
        }

        Ok(())
    }
}
