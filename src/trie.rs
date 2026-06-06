use std::{
    mem::{self, MaybeUninit},
    ptr,
};

use fxhash::FxHashMap;
use thiserror::Error;
use tracing::info;

use crate::{
    libsais::{LcpRmq, Text, TextError},
    time::time,
    trie::{
        divisor::Divisor,
        domain::{Domain, Domain16},
    },
};

mod divisor;
mod domain;

#[cfg_attr(
    feature = "rkyv",
    derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)
)]
#[derive(Clone, Debug)]
pub struct CompressedTrie<V> {
    nodes: Box<[CompressedNode]>,
    values: Box<[Value<V>]>,
    nibbles: Box<str>,
}

#[derive(Debug, Error)]
pub enum TrieError {
    #[error(transparent)]
    Text(#[from] TextError),
}

#[cfg_attr(
    feature = "rkyv",
    derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)
)]
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct TriePath(u64);

#[cfg_attr(
    feature = "rkyv",
    derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)
)]
#[derive(Clone, Debug)]
struct Value<V> {
    path: TriePath,
    value: V,
}

#[cfg_attr(
    feature = "rkyv",
    derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)
)]
#[derive(Clone, Debug)]
#[repr(C)]
struct CompressedNode {
    next_index: u32,
    child_count: u8,
    divisor: Divisor,
    domain: Domain16,
}

#[allow(non_camel_case_types)]
#[derive(Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
enum u4 {
    #[default]
    _0 = 0,
    _1 = 1,
    _2 = 2,
    _3 = 3,
    _4 = 4,
    _5 = 5,
    _6 = 6,
    _7 = 7,
    _8 = 8,
    _9 = 9,
    _10 = 10,
    _11 = 11,
    _12 = 12,
    _13 = 13,
    _14 = 14,
    _15 = 15,
}

impl<V> CompressedTrie<V> {
    pub fn new<I, K>(iter: I, paths: Option<&mut [TriePath]>) -> Result<Self, TrieError>
    where
        I: IntoIterator<Item = (K, V)>,
        K: AsRef<str>,
    {
        let iter = iter.into_iter();

        let mut values = Vec::with_capacity(iter.size_hint().0);

        let text = Text::new(iter.map(|(key, value)| {
            values.push(MaybeUninit::new(value));
            key
        }))?;

        let gsa = time!(text.gsa().expect("libsais error"), |t| info!(
            "compiled gsa ({t:.02?})"
        ));

        let isa = text.isa(&gsa);

        let lcp_rmq = time!(text.lcp_rmq(&gsa).expect("libsais error"), |t| info!(
            "compiled lcp and its rmq ({t:.02?})"
        ));

        let (keys, keys_lcp_rmq, keys_isa) = time!(
            text.strings_lcp_isa(&gsa, &lcp_rmq, &isa),
            |t| info!("compiled lcp, rmq and isa for keys ({t:.02?})")
        );

        let values = {
            let mut new_values = Box::new_uninit_slice(values.len());

            for (to, from) in keys
                .iter()
                .map(|index| keys_isa[index] as usize)
                .enumerate()
            {
                new_values[to] = unsafe { ptr::read(&values[from]) };
            }

            new_values
        };

        struct Builder<'a, V> {
            kv: (Box<[u32]>, Box<[MaybeUninit<V>]>),
            lcp_rmq: LcpRmq,
            isa: FxHashMap<u32, u32>,
            text: &'a Text,
        }

        impl<'a, V> Builder<'a, V> {
            fn new(
                keys: Box<[u32]>,
                values: Box<[MaybeUninit<V>]>,
                lcp_rmq: LcpRmq,
                isa: FxHashMap<u32, u32>,
                text: &'a Text,
            ) -> Self {
                Self {
                    kv: (keys, values),
                    lcp_rmq,
                    isa,
                    text,
                }
            }

            fn build(&self) {
                let len = self.kv.0.len();

                let mut graph = vec![(0, -1); len];

                self.visit_all(|i, j, lcp| {
                    if i == j {
                        return;
                    }

                    let (end, min_lcp) = &mut graph[i];

                    if *end > j && isize::cast_unsigned(*min_lcp) >= lcp {
                        *min_lcp = lcp as isize;
                        *end = j;
                    }
                });

                std::fs::write("graph3.txt", format!("{graph:#?}")).unwrap();
            }

            // fn build(&mut self) {
            //     let mut counter = 0;
            //     self.visit_all(|builder, range, lcp| {
            //         eprintln!("{counter}:");
            //         counter += 1;
            //         for index in &builder.kv.0[range] {
            //             eprintln!("{:->lcp$}{}", "", &self.text[*index + lcp as u32]);
            //         }
            //     });
            // }

            fn visit_all<F>(&self, mut f: F)
            where
                F: FnMut(usize, usize, usize),
            {
                if self.kv.0.is_empty() {
                    return;
                }

                let left = 0;
                let right = self.kv.0.len() - 1;

                self.visit(left, right, &mut f);
            }

            fn visit<F>(&self, left: usize, right: usize, f: &mut F)
            where
                F: FnMut(usize, usize, usize),
            {
                if left == right {
                    f(left, right, 0);
                    return;
                }

                let lcp_pos = self.lcp_rmq.range_min(
                    self.isa[&self.kv.0[left]] as usize + 1,
                    self.isa[&self.kv.0[right]] as usize,
                );

                let lcp = self.lcp_rmq[lcp_pos] as usize;
                f(left, right, lcp);

                self.visit(lcp_pos, right, f);
                self.visit(left, lcp_pos - 1, f);
            }
        }

        let mut builder = Builder::new(keys, values, keys_lcp_rmq, keys_isa, &text);

        time!(
            builder.build(),
            |t| info!("visited all strings ({t:.02?})",)
        );

        // fn visit(
        //     keys: &[u32],
        //     lcp_rmq: &LcpRmq,
        //     isa: &FxHashMap<u32, u32>,
        //     pos: usize,
        //     text: &Text,
        //     lcp: usize,
        // ) {
        //     eprint!("\n");
        //     for index in keys {
        //         eprintln!("{:->lcp$}{}", "", &text[*index + lcp as u32]);
        //     }

        //     let Some(right) = keys.last() else {
        //         return;
        //     };
        //     let left = &keys[0];

        //     if ptr::eq(left, right) {
        //         return;
        //     }

        //     let min_lcp_pos = lcp_rmq.range_min(isa[left] as usize + 1, isa[right] as usize);
        //     let min_lcp = lcp_rmq[min_lcp_pos] as usize;

        //     let mid = min_lcp_pos - pos;
        //     let (keys_left, keys_right) = keys.split_at(mid);

        //     visit(keys_right, lcp_rmq, isa, pos + mid, text, min_lcp);
        //     visit(keys_left, lcp_rmq, isa, pos, text, min_lcp);
        // }

        // time!(
        //     visit(&keys, &keys_lcp_rmq, &keys_isa, 0, &text, 0),
        //     |t| info!("visited all strings ({t:.02?})",)
        // );

        todo!()
    }

    pub fn get<K>(&self, key: K) -> Option<&V>
    where
        K: AsRef<str>,
    {
        let path = self.get_path(key)?;
        self.path_to_value(path)
    }

    pub fn get_path<K>(&self, key: K) -> Option<TriePath>
    where
        K: AsRef<str>,
    {
        let mut key = key.as_ref();
        let mut path = TriePath::default();

        if key.is_empty() {
            return Some(path);
        }

        let mut even_node = self.root();

        while even_node.next_index != 0 {
            let (lo, hi): (u4, u4) = key.as_bytes()[0].split();

            let child_index = even_node.domain.compress(hi)?;
            path.push(child_index, even_node.divisor);

            let odd_node_index = even_node.next_index as usize + child_index as usize;
            let odd_node = &self.nodes[odd_node_index];

            let nibble = self.nibble(odd_node.next_index);
            if !key.starts_with(nibble) {
                return None;
            }

            key = &key[nibble.len()..];
            if key.is_empty() {
                break;
            }

            let child_index = odd_node.domain.compress(lo)?;
            path.push(child_index, odd_node.divisor);

            let first_sibling_index = even_node.next_index as usize;

            let child_index = child_index as u8
                + self.nodes[first_sibling_index..odd_node_index]
                    .iter()
                    .map(|prev| prev.child_count)
                    .sum::<u8>();

            let last_sibling_index = first_sibling_index + even_node.child_count as usize;

            even_node = &self.nodes[last_sibling_index + child_index as usize];
        }

        key.is_empty().then_some(path)
    }

    pub fn path_to_value(&self, path: TriePath) -> Option<&V> {
        let index = self
            .values
            .binary_search_by(|value| value.path.cmp(&path))
            .ok()?;

        Some(&self.values[index].value)
    }

    pub fn path_to_key(&self, mut path: TriePath) -> String {
        if path.is_empty() {
            return "".to_string();
        }

        let mut even_node = self.root();
        let mut result = String::new();

        while even_node.next_index != 0 {
            let child_index = path.pop(even_node.divisor);

            let odd_node_index = even_node.next_index as usize + child_index as usize;
            let odd_node = &self.nodes[odd_node_index];

            let nibble = self.nibble(odd_node.next_index);
            result += nibble;

            let child_index = path.pop(odd_node.divisor);

            let first_sibling_index = even_node.next_index as usize;

            let child_index = child_index as u8
                + self.nodes[first_sibling_index..odd_node_index]
                    .iter()
                    .map(|prev| prev.child_count)
                    .sum::<u8>();

            let last_sibling_index = first_sibling_index + even_node.child_count as usize;

            even_node = &self.nodes[last_sibling_index + child_index as usize];
        }

        result
    }

    #[track_caller]
    fn root(&self) -> &CompressedNode {
        &self.nodes[0]
    }

    fn nibble(&self, index: u32) -> &str {
        self.nibbles[index as usize..].split_once('\0').unwrap().0
    }
}

impl TriePath {
    fn push(&mut self, child_index: u4, divisor: Divisor) {
        let mut child_index = child_index as u8;

        if !divisor.is_power_of_two() {
            child_index += 1;
        }
        debug_assert!(child_index < divisor as u8);

        self.0 = self.0.strict_mul(divisor as u64);
        self.0 += child_index as u64;
    }

    fn pop(&mut self, divisor: Divisor) -> u4 {
        let mut child_index;
        (self.0, child_index) = divisor.apply(self.0);

        if !divisor.is_power_of_two() {
            child_index -= 1;
        }

        unsafe { mem::transmute::<u8, u4>(child_index) }
    }

    fn is_empty(&self) -> bool {
        self.0 == 0
    }
}

impl CompressedNode {
    fn new() -> Self {
        Self {
            next_index: 0,
            child_count: 0,
            divisor: Divisor::_1,
            domain: 0,
        }
    }
}

impl<V, S> FromIterator<(S, V)> for CompressedTrie<V>
where
    S: AsRef<str>,
{
    fn from_iter<I: IntoIterator<Item = (S, V)>>(iter: I) -> Self {
        Self::new(iter, None).unwrap()
    }
}

trait Split: Sized {
    fn split(self) -> (u4, u4);
}

impl Split for u8 {
    #[inline(always)]
    fn split(self) -> (u4, u4) {
        let lo = unsafe { mem::transmute::<u8, u4>(self & 0b1111) };
        let hi = unsafe { mem::transmute::<u8, u4>(self >> 4) };
        (lo, hi)
    }
}

#[test]
fn trie() {
    CompressedTrie::new(
        [
            ("action/script/modifier.hks", 26),
            ("action/statenameid.txt", 22),
            ("event/m54_00_00_00.emevd.dcx", 28),
            ("parts/am_f_7450_l.partsbnd.dcx", 30),
        ],
        None,
    )
    .unwrap();
}
