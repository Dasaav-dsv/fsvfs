const PRIME_32: u32 = 37;
const PRIME_64: u64 = 133;

const CHUNK_SIZE: usize = 8;

impl_mad_hash! {
    pub fn mad_hash32<PRIME_32, CHUNK_SIZE>(bytes: &[u8]) -> u32;
    pub fn mad_hash64<PRIME_64, CHUNK_SIZE>(bytes: &[u8]) -> u64;
}

// Big thanks to sfix and tremwil for the basis of this implementation.
macro_rules! impl_mad_hash {
    (
        $(
            $(#[$m:meta])*
            $v:vis fn $f:ident<$p:ident, $n:ident>($b:ident: &[u8]) -> $t:ty;
        )+
    ) => {
        $(
            $(#[$m])*
            $v fn $f($b: impl AsRef<[u8]>) -> $t {
                const P: $t = $p as $t;
                const N: usize = $n as usize;
                const P_TO_N: $t = P.wrapping_pow(N as u32);

                const POWERS: [$t; N] = {
                    let mut powers = [0; N];
                    let mut i = 0;
                    while i < N {
                        powers[i] = P.wrapping_pow((N - i) as u32 - 1);
                        i += 1;
                    }
                    powers
                };

                let mut hash: $t = 0;
                let mut chunks = $b.as_ref().chunks_exact(N);

                for chunk in chunks.by_ref() {
                    let dot_product = (0..N).fold(0, |dot, i| {
                        let val = (normalize(chunk[i]) as $t).wrapping_mul(POWERS[i]);
                        <$t>::wrapping_add(dot, val)
                    });

                    hash = hash.wrapping_mul(P_TO_N).wrapping_add(dot_product);
                }

                for b in chunks.remainder() {
                    hash = hash.wrapping_mul(P).wrapping_add(*b as $t);
                }

                hash
            }
        )+
    };
}

pub(self) use impl_mad_hash;

#[inline(always)]
fn normalize(mut c: u8) -> u8 {
    if c >= b'A' && c <= b'Z' {
        c |= 32;
    }

    if c == b'\\' {
        c = b'/';
    }

    c
}

#[cfg(test)]
mod tests {
    use std::fs;

    use crate::dvdbnd::hash::{PRIME_32, PRIME_64, mad_hash32, mad_hash64, normalize};

    #[test]
    fn hash() {
        let data5 = fs::read_to_string("dist/dvdbnd/Hash/DarkSouls3_PC/Data5.txt").unwrap();

        for line in data5.lines() {
            assert_eq!(mad_hash32(line), mad_hash32_naive(line), "{line}");
            assert_eq!(mad_hash64(line), mad_hash64_naive(line), "{line}");
        }
    }

    fn mad_hash32_naive(bytes: impl AsRef<[u8]>) -> u32 {
        bytes.as_ref().iter().fold(0, |hash, byte| {
            hash.wrapping_mul(PRIME_32)
                .wrapping_add(normalize(*byte) as u32)
        })
    }

    fn mad_hash64_naive(bytes: impl AsRef<[u8]>) -> u64 {
        bytes.as_ref().iter().fold(0, |hash, byte| {
            hash.wrapping_mul(PRIME_64)
                .wrapping_add(normalize(*byte) as u64)
        })
    }
}
