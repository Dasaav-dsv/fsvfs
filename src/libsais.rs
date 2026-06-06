use std::ops::{self, Bound};

use fxhash::FxHashMap;
use thiserror::Error;
use vers_vecs::FastRmq;

mod sys;

#[derive(Clone, Debug)]
pub struct Text {
    inner: Box<str>,
    count: usize,
}

#[derive(Debug, Error)]
pub enum TextError {
    #[error("text is bigger than 2 GB")]
    TooLong,

    #[error("input contains empty strings (\"\")")]
    HasEmpty,

    #[error("input contains nul characters ('\\0')")]
    HasNuls,
}

#[derive(Debug, Error)]
pub enum LibsaisError {
    #[error("library error: invalid parameter")]
    InvalidParameter,

    #[error("library error: internal error (code {0})")]
    Internal(i32),
}

pub type Gsa = Box<[u32]>;

pub type Isa = Box<[u32]>;

pub type LcpRmq = FastRmq;

impl Text {
    pub fn new<I>(iter: I) -> Result<Self, TextError>
    where
        I: IntoIterator<Item: AsRef<str>>,
    {
        let iter = iter.into_iter();

        // Lower bound (n one byte strings <=> 2n bytes).
        let lower_bound = iter.size_hint().0 * 2;

        // Insert an extra nul at the start.
        let mut text = String::with_capacity(lower_bound + 1);
        text.push('\0');

        let mut count = 0;

        for str in iter {
            let str = str.as_ref();

            if str.is_empty() {
                return Err(TextError::HasEmpty);
            } else if str.contains('\0') {
                return Err(TextError::HasNuls);
            }

            text.push_str(str);
            text.push('\0');

            count += 1;
        }

        if text.len() - 1 > i32::MAX as usize {
            return Err(TextError::TooLong);
        }

        Ok(Self {
            inner: text.into_boxed_str(),
            count,
        })
    }

    pub fn get(&self, index: u32) -> Option<&str> {
        let start = self.text().get(index as usize..)?;
        Some(start.split_once('\0')?.0)
    }

    pub fn prefix(&self, index: u32, len: u64) -> &str {
        let index = index as usize;
        &self.text()[index..index + len as usize]
    }

    pub fn is_nul(&self, index: u32) -> bool {
        self.inner.as_bytes()[index as usize] == b'\0'
    }

    pub fn prefix_len(&self, index: u32) -> Option<usize> {
        let start = self.text().get(index as usize..)?;
        start.find('\0')
    }

    #[allow(non_snake_case)]
    pub fn gsa(&self) -> Result<Gsa, LibsaisError> {
        let text = self.text();
        let len = text.len();

        let mut suffix_array = vec![0; len].into_boxed_slice();

        if len == 0 {
            return Ok(suffix_array);
        }

        let T = text.as_ptr();
        let SA = suffix_array.as_mut_ptr() as *mut i32;

        let n = i32::try_from(len).unwrap();
        let fs = 0;
        let freq = None;

        unsafe {
            sys::gsa(T, SA, n, fs, freq)?;
        }

        Ok(suffix_array)
    }

    #[allow(non_snake_case)]
    pub fn lcp_rmq(&self, gsa: &Gsa) -> Result<LcpRmq, LibsaisError> {
        let text = self.text();
        let len = text.len();

        assert_eq!(len, gsa.len(), "suffix array must belong to this text");

        if len == 0 {
            return Ok(LcpRmq::from_vec(vec![]));
        }

        let rounded_len = (len + 1) & !1;
        let mut lcp_plcp = vec![0u64; rounded_len];

        let T = text.as_ptr();
        let SA = gsa.as_ptr() as *const i32;
        let LCP = lcp_plcp[..rounded_len / 2].as_mut_ptr() as *mut i32;
        let PLCP = lcp_plcp[rounded_len / 2..].as_mut_ptr() as *mut i32;

        let n = i32::try_from(len).unwrap();

        unsafe {
            sys::plcp_gsa(T, SA, PLCP, n)?;
            sys::lcp(PLCP, SA, LCP, n)?;
        }

        let ptr = lcp_plcp.as_mut_ptr();
        for i in (0..len).rev() {
            unsafe {
                let full = ptr.add(i);
                let half = *ptr.cast::<u32>().add(i);

                *full = half as u64;
            }
        }

        lcp_plcp.truncate(len);
        let lcp = lcp_plcp;

        Ok(LcpRmq::from_vec(lcp))
    }

    pub fn isa(&self, gsa: &Gsa) -> Isa {
        let mut isa = vec![0; gsa.len()].into_boxed_slice();

        for (&index, rev_index) in gsa.iter().zip(0..) {
            isa[index as usize] = rev_index;
        }

        isa
    }

    pub fn strings_lcp_isa(
        &self,
        gsa: &Gsa,
        lcp: &LcpRmq,
        isa: &Isa,
    ) -> (Box<[u32]>, LcpRmq, FxHashMap<u32, u32>) {
        let len = self.text().len();

        assert_eq!(len, gsa.len(), "suffix array must belong to this text");
        assert_eq!(len, lcp.len(), "LCP array must belong to this text");

        let mut strings = Vec::with_capacity(self.count);
        let mut strings_isa = FxHashMap::with_capacity_and_hasher(self.count, Default::default());

        strings.extend(
            gsa.iter()
                .filter(|index| self.is_nul(**index))
                .zip(0..)
                .map(|(&index, rev_index)| {
                    strings_isa.insert(index, rev_index);
                    index
                }),
        );

        let mut strings_lcp = Vec::with_capacity(self.count);
        strings_lcp.push(0u64);

        strings_lcp.extend(strings.array_windows().map(|&[a, b]| {
            lcp[lcp.range_min_with_range((
                Bound::Excluded(isa[a as usize] as usize),
                Bound::Included(isa[b as usize] as usize),
            ))]
        }));

        (
            strings.into_boxed_slice(),
            LcpRmq::from_vec(strings_lcp),
            strings_isa,
        )
    }

    pub fn strings_2(&self, gsa: &Gsa) -> Box<[u32]> {
        assert_eq!(
            self.text().len(),
            gsa.len(),
            "suffix array must belong to this text"
        );

        let mut strings = Vec::with_capacity(self.count);
        strings.extend(gsa.iter().filter(|index| self.is_nul(**index)));

        strings.into_boxed_slice()
    }

    pub fn count_repeats(&self, gsa: &Gsa, lcp: &LcpRmq) -> Box<[u32]> {
        let len = self.text().len();

        assert_eq!(len, gsa.len(), "suffix array must belong to this text");
        assert_eq!(len, lcp.len(), "LCP array must belong to this text");

        let mut repeats = vec![0; len].into_boxed_slice();
        let mut repeats_i = 0;

        for ((i, &lcp), &[a, b]) in lcp.iter().enumerate().skip(1).zip(gsa.array_windows()) {
            if self.prefix(a + lcp as u32, 1) != "\0" || self.prefix(b + lcp as u32, 1) != "\0" {
                let repeats = &mut repeats[repeats_i..i];
                let repeats_len = repeats.len() as u32;

                for repeat in repeats {
                    *repeat = repeats_len;
                }

                repeats_i = i;
            }
        }

        repeats
    }

    pub fn strings(&self) -> impl Iterator<Item = (usize, &str)> {
        let mut i = 0;
        self.text().split_terminator('\0').map(move |str| {
            let next_i = i;
            i += str.len() + 1;
            (next_i, str)
        })
    }

    fn text(&self) -> &str {
        // Trim the leading nul.
        // SAFETY: it is assumed the first byte is always '\0'.
        unsafe { self.inner.get_unchecked(1..) }
    }
}

impl ops::Index<u32> for Text {
    type Output = str;

    #[inline]
    fn index(&self, index: u32) -> &Self::Output {
        self.get(index).unwrap()
    }
}

impl ops::Index<(u32, u64)> for Text {
    type Output = str;

    #[inline]
    fn index(&self, (index, len): (u32, u64)) -> &Self::Output {
        self.prefix(index, len)
    }
}
