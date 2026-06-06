use std::hint::assert_unchecked;

#[cfg_attr(
    feature = "rkyv",
    derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum Divisor {
    _1 = 1,
    _2 = 2,
    _3 = 3,
    _4 = 4,
    _5 = 5,
    _7 = 7,
    _8 = 8,
    _11 = 11,
    _13 = 13,
    _16 = 16,
    _17 = 17,
}

// x as u64 / y <=> x * rdiv / 2^(64 + shr)
// rdiv = 2^(64 + shr) / y
// shr = ceil(lg2(y))
//
// x as u64 / 3 <=> (x * 0x1_5555_5555_5555_5555) >> 66
// x as u64 / 5 <=> (x * 0x1_9999_9999_9999_999a) >> 67
// x as u64 / 7 <=> (x * 0x1_2492_4924_9249_2492) >> 67
// x as u64 / 11 <=> (x * 0x1_745d_1745_d174_5d17) >> 68
// x as u64 / 13 <=> (x * 0x1_3b13_b13b_13b1_3b14) >> 68
// x as u64 / 17 <=> (x * 0x1_e1e1_e1e1_e1e1_e1e2) >> 69
static CONSTS: ([u64; 15], [u8; 15]) = (
    [
        0x5555_5555_5555_5555,
        0,
        0x9999_9999_9999_999a,
        0,
        0x2492_4924_9249_2492,
        0,
        0,
        0,
        0x745d_1745_d174_5d17,
        0,
        0x3b13_b13b_13b1_3b14,
        0,
        0,
        0,
        0xe1e1_e1e1_e1e1_e1e2,
    ],
    [2, 0, 3, 0, 3, 0, 0, 0, 4, 0, 4, 0, 0, 0, 5],
);

impl Divisor {
    #[inline]
    pub fn apply(self, dividend: u64) -> (u64, u8) {
        if matches!(self, Self::_1) {
            (dividend, 0)
        } else if self.is_power_of_two() {
            // The compiler has enough knowledge to ilog2 and then shift right.
            let quotient = dividend / self as u64;
            let remainder = dividend % self as u64;

            (quotient, remainder as u8)
        } else {
            // SAFETY: Divisor::_3 is the first variant that is not a power of two.
            unsafe {
                assert_unchecked(self >= Divisor::_3);
            }

            // Adjust to the start of the table.
            let index = self as usize - Self::_3 as usize;

            // `rdiv_lo` is mod 2^64, add the high part (always 1 * 2^64) below.
            let (rdiv_lo, shr) = (CONSTS.0[index], CONSTS.1[index]);

            // Quotient / 2^64:
            let quotient_hi = (dividend as u128 * rdiv_lo as u128) >> 64;

            // Add dividend * 1 * 2^64 to the high part / 2^64 and divide by 2^shr.
            let quotient = (dividend + quotient_hi as u64) >> shr;
            let remainder = dividend - self as u64 * quotient;

            (quotient, remainder as u8)
        }
    }

    #[inline]
    pub fn is_power_of_two(self) -> bool {
        (self as u8).is_power_of_two()
    }
}
