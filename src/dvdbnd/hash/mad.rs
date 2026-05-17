const PRIME_32: u32 = 37;
const PRIME_64: u64 = 133;

pub trait MadHash<const N: usize>: MulAdd {
    const PRIME: Self;
    const POWERS: [Self; N];
    const POWERN: Self;
}

// Big thanks to sfix and tremwil for the basis of this implementation.
pub fn mad_hash<const N: usize, T>(init: T, bytes: &[u8]) -> T
where
    T: MadHash<N> + Copy + From<u8>,
{
    let mut hash = init;
    let mut chunks = bytes.chunks_exact(N);

    for chunk in chunks.by_ref() {
        let dot_product = (0..N).fold(T::from(0), |dot, i| {
            let val = T::from(normalize(chunk[i])).wrapping_mul(T::POWERS[i]);
            T::wrapping_add(dot, val)
        });

        hash = hash.wrapping_mul(T::POWERN).wrapping_add(dot_product);
    }

    for b in chunks.remainder() {
        hash = hash
            .wrapping_mul(T::PRIME)
            .wrapping_add(T::from(normalize(*b)));
    }

    hash
}

#[inline(always)]
pub const fn mad_hash32(init: u32, bytes: &[u8]) -> u32 {
    let mut i = 0;
    let mut hash = init;
    while i < bytes.len() {
        hash = hash
            .wrapping_mul(PRIME_32)
            .wrapping_add(normalize(bytes[i]) as u32);
        i += 1;
    }
    hash
}

#[inline(always)]
pub const fn mad_hash64(init: u64, bytes: &[u8]) -> u64 {
    let mut i = 0;
    let mut hash = init;
    while i < bytes.len() {
        hash = hash
            .wrapping_mul(PRIME_64)
            .wrapping_add(normalize(bytes[i]) as u64);
        i += 1;
    }
    hash
}

#[inline(always)]
const fn normalize(c: u8) -> u8 {
    match c {
        b'A'..=b'Z' => c | 32,
        b'\\' => b'/',
        _ => c,
    }
}

impl<const N: usize> MadHash<N> for u32 {
    const PRIME: Self = PRIME_32;
    const POWERS: [Self; N] = impl_powers!(PRIME_32);
    const POWERN: Self = <Self as MadHash<N>>::PRIME.wrapping_pow(N as u32);
}

impl<const N: usize> MadHash<N> for u64 {
    const PRIME: Self = PRIME_64;
    const POWERS: [Self; N] = impl_powers!(PRIME_64);
    const POWERN: Self = <Self as MadHash<N>>::PRIME.wrapping_pow(N as u32);
}

macro_rules! impl_powers {
    ($p:ident) => {{
        let mut powers = [1; N];
        let mut i = N - 1;
        while i > 0 {
            powers[i - 1] = $p.wrapping_mul(powers[i]);
            i -= 1;
        }
        powers
    }};
}

use impl_powers;

pub trait MulAdd: Sized {
    fn wrapping_add(self, rhs: Self) -> Self;
    fn wrapping_mul(self, rhs: Self) -> Self;
}

macro_rules! impl_muladd {
    ($($t:path$(,)?)+) => {
        $(
            impl MulAdd for $t {
                #[inline(always)]
                fn wrapping_add(self, rhs: Self) -> Self {
                    <$t>::wrapping_add(self, rhs)
                }
                #[inline(always)]
                fn wrapping_mul(self, rhs: Self) -> Self {
                    <$t>::wrapping_mul(self, rhs)
                }
            }
        )+
    };
}

impl_muladd! { u32, u64 }

#[cfg(test)]
mod tests {
    use std::fs;

    use crate::dvdbnd::hash::mad::{mad_hash, mad_hash32, mad_hash64};

    #[test]
    fn hash() {
        let data5 = fs::read_to_string("dist/dvdbnd/Hash/DarkSouls3_PC/Data5.txt").unwrap();

        for line in data5.lines() {
            let bytes = line.as_bytes();

            let hash32 = mad_hash32(0, bytes);
            assert_eq!(hash32, mad_hash::<4, _>(0, bytes), "{line}");
            assert_eq!(hash32, mad_hash::<8, _>(0, bytes), "{line}");

            let hash64 = mad_hash64(0, bytes);
            assert_eq!(hash64, mad_hash::<4, _>(0, bytes), "{line}");
            assert_eq!(hash64, mad_hash::<8, _>(0, bytes), "{line}");
        }
    }
}
