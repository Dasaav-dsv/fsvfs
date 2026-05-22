use zerocopy::{FromBytes, Immutable, KnownLayout, TryFromBytes, Unaligned};

use crate::unaligned::{U16, U24, U32};

#[cfg_attr(feature = "rkyv", derive(rkyv::Archive, rkyv::Serialize))]
#[derive(Clone, Copy, Debug, KnownLayout, Immutable, Unaligned, TryFromBytes)]
#[repr(u8)]
enum EncryptionHeader {
    Aes128EcbNone(Aes128),
}

#[cfg_attr(feature = "rkyv", derive(rkyv::Archive, rkyv::Serialize))]
#[derive(Clone, Copy, Debug, KnownLayout, Immutable, Unaligned, FromBytes)]
#[repr(transparent)]
struct Aes128(Aes<16>);

#[cfg_attr(feature = "rkyv", derive(rkyv::Archive, rkyv::Serialize))]
#[derive(Clone, Copy, Debug, KnownLayout, Immutable, Unaligned, FromBytes)]
#[repr(C, align(1))]
struct Aes<const N: usize> {
    key: [u8; N],
    range_count: U24,
}

#[cfg_attr(feature = "rkyv", derive(rkyv::Archive, rkyv::Serialize))]
#[derive(Clone, Copy, Debug, KnownLayout, Immutable, Unaligned, FromBytes)]
#[repr(C, align(1))]
struct EncryptionRange {
    start_offset: U32,
    block_count: U16,
}
