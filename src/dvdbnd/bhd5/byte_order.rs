use std::{fmt, marker::PhantomData};

use zerocopy::{BE, Immutable, KnownLayout, LE, TryFromBytes, Unaligned};

use crate::dvdbnd::bhd5::consts::{BigEndian, LittleEndian, One, Zero};

#[derive(Clone, Copy, Default, Debug, KnownLayout, Immutable, Unaligned, TryFromBytes)]
#[repr(C, align(1))]
pub struct Bom<O: ByteOrderExt>(O::BomValue);

#[derive(Clone, Copy, Default, Debug, KnownLayout, Immutable, Unaligned, TryFromBytes)]
#[repr(C, align(1))]
pub struct ZeroU32<O: ByteOrderExt>(Zero, Zero, Zero, Zero, PhantomData<O>);

#[derive(Clone, Copy, Default, Debug, KnownLayout, Immutable, Unaligned, TryFromBytes)]
#[repr(C, align(1))]
pub struct OneU32<O: ByteOrderExt>(O::OneU32Value);

#[derive(Clone, Copy, Default, Debug, KnownLayout, Immutable, Unaligned, TryFromBytes)]
#[repr(C, align(1))]
pub struct OneU32LE(One, Zero, Zero, Zero);

#[derive(Clone, Copy, Default, Debug, KnownLayout, Immutable, Unaligned, TryFromBytes)]
#[repr(C, align(1))]
pub struct OneU32BE(Zero, Zero, Zero, One);

pub trait ByteOrderExt: zerocopy::byteorder::ByteOrder {
    type BomValue: ByteOrderValue;
    type OneU32Value: ByteOrderValue;
}

impl ByteOrderExt for LE {
    type BomValue = LittleEndian;
    type OneU32Value = OneU32LE;
}

impl ByteOrderExt for BE {
    type BomValue = BigEndian;
    type OneU32Value = OneU32BE;
}

pub trait ByteOrderValue:
    Clone + Copy + fmt::Debug + KnownLayout + Immutable + Unaligned + TryFromBytes
{
}

impl<T> ByteOrderValue for T where
    T: Clone + Copy + fmt::Debug + KnownLayout + Immutable + Unaligned + TryFromBytes
{
}
