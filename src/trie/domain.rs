use std::mem;

use crate::trie::u4;

pub type Domain16 = u16;

pub(super) trait Domain {
    type Value;

    fn include(&mut self, value: Self::Value);

    fn compress(&self, value: Self::Value) -> Option<Self::Value>;
}

impl Domain for Domain16 {
    type Value = u4;

    #[inline]
    fn include(&mut self, value: Self::Value) {
        *self |= 1 << value as u8;
    }

    #[inline]
    fn compress(&self, value: Self::Value) -> Option<Self::Value> {
        match gather(1 << value as u8, *self) {
            0 => None,
            in_image => {
                let value = in_image.trailing_zeros() as u8;
                // SAFETY: in_image is not 0, so value is 0..=15
                unsafe { Some(mem::transmute::<u8, u4>(value)) }
            }
        }
    }
}

#[inline(always)]
fn gather(x: u16, mask: u16) -> u16 {
    cfg_select! {
        all(target_arch = "x86_64", target_feature = "bmi2") => unsafe {
            std::arch::x86_64::_pext_u32(x as u32, mask as u32) as u16
        }
        all(target_arch = "x86", target_feature = "bmi2") => unsafe {
            std::arch::x86::_pext_u32(x as u32, mask as u32) as u16
        }
        _ => gather_impl(x, mask)
    }
}

// Taken from https://github.com/rust-lang/rust/pull/149663 and https://github.com/rust-lang/libs-team/issues/695#issuecomment-3538922128
// Original by @ocaneco https://github.com/okaneco
const STAGES: usize = u16::BITS.ilog2() as usize;
#[inline(always)]
const fn prepare(sparse: u16) -> [u16; STAGES] {
    // We'll start with `zeros` as a mask of the bits to be removed,
    // and compute into `masks` the parts that shift at each stage.
    let mut zeros = !sparse;
    let mut masks = [0; STAGES];
    let mut stage = 0;
    while stage < STAGES {
        let n = 1 << stage;
        // Suppose `zeros` has bits set at ranges `{ a..a+n, b..b+n, ... }`.
        // Then `parity` will be computed as `{ a.. } XOR { b.. } XOR ...`,
        // which will be the ranges `{ a..b, c..d, e.. }`.
        let mut parity = zeros;
        let mut len = n;
        while len < u16::BITS {
            parity ^= parity << len;
            len <<= 1;
        }
        masks[stage] = parity;

        // Toggle off the bits that are shifted into:
        // { a..a+n, b..b+n, ... } & !{ a..b, c..d, e.. }
        // == { b..b+n, d..d+n, ... }
        zeros &= !parity;
        // Expand the remaining ranges down to the bits that were
        // shifted from: { b-n..b+n, d-n..d+n, ... }
        zeros ^= zeros >> n;

        stage += 1;
    }
    masks
}

#[allow(unused)]
#[inline(always)]
const fn gather_impl(mut x: u16, sparse: u16) -> u16 {
    let masks = prepare(sparse);
    x &= sparse;
    let mut stage = 0;
    while stage < STAGES {
        let n = 1 << stage;
        // Consider each two runs of data with their leading
        // groups of `n` 0-bits. Suppose that the run that is
        // shifted right has length `a`, and the other one has
        // length `b`. Assume that only zeros are shifted in.
        // ```text
        // [0; n], [X; a], [0; n], [Y; b] // x
        // [0; n], [X; a], [0; n], [0; b] // q
        // [0; n], [0; a   +   n], [Y; b] // x ^= q
        // [0; n   +   n], [X; a], [0; b] // q >> n
        // [0; n], [0; n], [X; a], [Y; b] // x ^= q << n
        // ```
        // Only zeros are shifted out, satisfying the assumption
        // for the next group.

        // In effect, the upper run of data is swapped with the
        // group of `n` zeros below it.
        let q = x & masks[stage];
        x ^= q;
        x ^= q >> n;

        stage += 1;
    }
    x
}
