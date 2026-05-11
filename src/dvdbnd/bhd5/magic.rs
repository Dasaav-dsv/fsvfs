use zerocopy::{Immutable, KnownLayout, TryFromBytes, Unaligned};

use crate::dvdbnd::bhd5::consts::{_5, B, D, H};

#[derive(Clone, Copy, Default, Debug, KnownLayout, Immutable, Unaligned, TryFromBytes)]
#[repr(C)]
pub struct Bhd5Magic(B, H, D, _5);
