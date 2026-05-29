use std::{
    cmp::Ordering,
    fmt,
    hash::{Hash, Hasher},
    marker::PhantomData,
    mem,
    ops::ControlFlow,
    slice,
};

use hashbrown::Equivalent;

use crate::filesystem::readonly::{Config, DefaultConfig, Normalize};

#[repr(transparent)]
pub struct Components<S, C = DefaultConfig>(pub PhantomData<C>, pub [S]);

#[derive(Clone, Copy, Default, Debug)]
#[repr(transparent)]
pub struct ComponentStr<'a, C = DefaultConfig>(pub PhantomData<C>, pub &'a str);

pub struct ComponentsIter<'a, S, C> {
    components: &'a [S],
    pos: usize,
    start_pos: usize,
    _marker: PhantomData<C>,
}

pub trait AsComponents {
    type S: AsRef<str> + Sized;

    fn as_components<C: Config>(&self) -> &Components<Self::S, C>;
}

impl<S> AsComponents for S
where
    S: AsRef<str>,
{
    type S = S;

    #[inline]
    fn as_components<C: Config>(&self) -> &Components<S, C> {
        Components::from_str(self)
    }
}

impl<S> AsComponents for [S]
where
    S: AsRef<str>,
{
    type S = S;

    #[inline]
    fn as_components<C: Config>(&self) -> &Components<S, C> {
        Components::from_slice(self)
    }
}

impl<S, C> Components<S, C>
where
    S: AsRef<str>,
    C: Config,
{
    #[inline]
    pub const fn from_str(s: &S) -> &Self {
        Self::from_slice(slice::from_ref(s))
    }

    #[inline]
    pub const fn from_slice(s: &[S]) -> &Self {
        // SAFETY: transmute to transparent wrapper.
        unsafe { mem::transmute::<&[S], &Self>(s) }
    }

    #[inline]
    pub fn iter(&self) -> ComponentsIter<'_, S, C> {
        ComponentsIter {
            components: &self.1,
            pos: 0,
            start_pos: 0,
            _marker: PhantomData,
        }
    }
}

impl<'a, C> ComponentStr<'a, C> {
    #[inline]
    pub fn new<S>(s: &'a S) -> Self
    where
        S: AsRef<str> + ?Sized,
    {
        Self(PhantomData, s.as_ref())
    }

    #[inline]
    pub fn as_components(&self) -> &Components<&str, C>
    where
        C: Config,
    {
        self.1.as_components()
    }
}

impl<S, C0, C1> Equivalent<ComponentStr<'_, C1>> for Components<S, C0>
where
    S: AsRef<str>,
    C0: Config,
    C1: Config,
{
    #[inline]
    fn equivalent(&self, key: &ComponentStr<'_, C1>) -> bool {
        self == key.as_components()
    }
}

impl<'a, S, C> IntoIterator for &'a Components<S, C>
where
    S: AsRef<str>,
    C: Config,
{
    type Item = &'a str;
    type IntoIter = ComponentsIter<'a, S, C>;

    #[inline]
    fn into_iter(self) -> Self::IntoIter {
        ComponentsIter {
            components: &self.1,
            pos: 0,
            start_pos: 0,
            _marker: PhantomData,
        }
    }
}

impl<'a, S, C> Iterator for ComponentsIter<'a, S, C>
where
    S: AsRef<str> + 'a,
    C: Config,
{
    type Item = &'a str;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let components = self.components.get(self.pos)?.as_ref();
            let components_start = &components[self.start_pos..];

            let component = match components_start.find(C::SEPARATORS) {
                Some(end_pos) => {
                    self.start_pos += end_pos + 1;
                    &components_start[..end_pos]
                }
                None => {
                    self.pos += 1;
                    self.start_pos = 0;
                    components_start
                }
            };

            if !matches!(component, "" | ".") {
                break Some(component);
            }
        }
    }
}

impl<S0, S1, C1, C0> PartialEq<Components<S1, C1>> for Components<S0, C0>
where
    S0: AsRef<str>,
    S1: AsRef<str>,
    C0: Config,
    C1: Config,
{
    #[inline]
    fn eq(&self, other: &Components<S1, C1>) -> bool {
        if const {
            matches!(C0::NORMALIZATION, Normalize::AsciiCase)
                | matches!(C1::NORMALIZATION, Normalize::AsciiCase)
        } {
            match iter_compare(self, other, |a, b| match a.eq_ignore_ascii_case(b) {
                true => ControlFlow::Continue(()),
                false => ControlFlow::Break(()),
            }) {
                ControlFlow::Continue(Ordering::Equal) => true,
                _ => false,
            }
        } else {
            self.iter().eq(other)
        }
    }
}

impl<S, C> Eq for Components<S, C>
where
    S: AsRef<str>,
    C: Config,
{
}

impl<S0, S1, C1, C0> PartialOrd<Components<S1, C1>> for Components<S0, C0>
where
    S0: AsRef<str>,
    S1: AsRef<str>,
    C0: Config,
    C1: Config,
{
    #[inline]
    fn partial_cmp(&self, other: &Components<S1, C1>) -> Option<Ordering> {
        if const {
            matches!(C0::NORMALIZATION, Normalize::AsciiCase)
                | matches!(C1::NORMALIZATION, Normalize::AsciiCase)
        } {
            match iter_compare(self, other, |a, b| match cmp_ignore_ascii_case(a, b) {
                Ordering::Equal => ControlFlow::Continue(()),
                non_eq => ControlFlow::Break(non_eq),
            }) {
                ControlFlow::Continue(ord) => Some(ord),
                ControlFlow::Break(ord) => Some(ord),
            }
        } else {
            self.iter().partial_cmp(other)
        }
    }
}

impl<S, C> Ord for Components<S, C>
where
    S: AsRef<str>,
    C: Config,
{
    #[inline]
    fn cmp(&self, other: &Self) -> Ordering {
        if const { matches!(C::NORMALIZATION, Normalize::AsciiCase) } {
            match iter_compare(self, other, |a, b| match cmp_ignore_ascii_case(a, b) {
                Ordering::Equal => ControlFlow::Continue(()),
                non_eq => ControlFlow::Break(non_eq),
            }) {
                ControlFlow::Continue(ord) => ord,
                ControlFlow::Break(ord) => ord,
            }
        } else {
            self.iter().cmp(other)
        }
    }
}

impl<S, C> Hash for Components<S, C>
where
    S: AsRef<str>,
    C: Config,
{
    #[inline]
    fn hash<H: Hasher>(&self, state: &mut H) {
        const CHUNK_SIZE: usize = 16;

        self.iter().for_each(|s| {
            state.write_usize(s.len());

            let (chunks, rem) = s.as_bytes().as_chunks::<CHUNK_SIZE>();

            for mut chunk in chunks.iter().cloned() {
                chunk.make_ascii_lowercase();
                Hash::hash_slice(&chunk, state);
            }

            if !rem.is_empty() {
                let mut last_chunk = [0; CHUNK_SIZE];
                last_chunk[..rem.len()].copy_from_slice(rem);
                last_chunk.make_ascii_lowercase();
                Hash::hash_slice(&last_chunk, state);
            }
        });
    }
}

impl<C> PartialEq for ComponentStr<'_, C>
where
    C: Config,
{
    #[inline]
    fn eq(&self, other: &ComponentStr<'_, C>) -> bool {
        self.as_components().eq(other.as_components())
    }
}

impl<C> Eq for ComponentStr<'_, C> where C: Config {}

impl<C> PartialOrd for ComponentStr<'_, C>
where
    C: Config,
{
    #[inline]
    fn partial_cmp(&self, other: &ComponentStr<'_, C>) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<C> Ord for ComponentStr<'_, C>
where
    C: Config,
{
    #[inline]
    fn cmp(&self, other: &Self) -> Ordering {
        self.as_components().cmp(other.as_components())
    }
}

impl<C> Hash for ComponentStr<'_, C>
where
    C: Config,
{
    #[inline]
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.as_components().hash(state);
    }
}

impl<S, C> fmt::Debug for Components<S, C>
where
    S: AsRef<str>,
    C: Config,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for component in self {
            f.write_str(component)?;
        }
        Ok(())
    }
}

// Taken from the Rust standard library `core::src::iter::traits::iter_compare`.
#[inline]
fn iter_compare<A, B, F, T>(a: A, b: B, f: F) -> ControlFlow<T, Ordering>
where
    A: IntoIterator,
    B: IntoIterator,
    F: FnMut(A::Item, B::Item) -> ControlFlow<T>,
{
    #[inline]
    fn compare<'a, B, X, T>(
        b: &'a mut B,
        mut f: impl FnMut(X, B::Item) -> ControlFlow<T> + 'a,
    ) -> impl FnMut(X) -> ControlFlow<ControlFlow<T, Ordering>> + 'a
    where
        B: Iterator,
    {
        move |x| match b.next() {
            None => ControlFlow::Break(ControlFlow::Continue(Ordering::Greater)),
            Some(y) => f(x, y).map_break(ControlFlow::Break),
        }
    }

    let mut a = a.into_iter();
    let mut b = b.into_iter();

    match a.try_for_each(compare(&mut b, f)) {
        ControlFlow::Continue(()) => ControlFlow::Continue(match b.next() {
            None => Ordering::Equal,
            Some(_) => Ordering::Less,
        }),
        ControlFlow::Break(x) => x,
    }
}

#[inline]
pub fn cmp_ignore_ascii_case(a: &str, b: &str) -> Ordering {
    let a = a.as_bytes();
    let b = b.as_bytes();

    a.len().cmp(&b.len()).then_with(|| {
        let cmp = a.iter().zip(b).try_for_each(|(a, b)| {
            match a.to_ascii_lowercase().cmp(&b.to_ascii_lowercase()) {
                Ordering::Equal => ControlFlow::Continue(()),
                non_eq => ControlFlow::Break(non_eq),
            }
        });

        match cmp {
            ControlFlow::Continue(()) => Ordering::Equal,
            ControlFlow::Break(non_eq) => non_eq,
        }
    })
}

#[cfg(test)]
mod tests {
    use std::{cmp::Ordering, hash::BuildHasher};

    use fxhash::FxBuildHasher;

    use crate::filesystem::{
        paths::components::{self, AsComponents},
        readonly::{Config, Normalize},
    };

    struct NormalizeAscii;
    impl Config for NormalizeAscii {
        const NORMALIZATION: Normalize = Normalize::AsciiCase;
        const SEPARATORS: &[char] = &['/', '\\'];
    }

    type Components<S> = components::Components<S, NormalizeAscii>;

    const PATHS: [&str; 11] = [
        "/model/map/t50_38_00_00.tpfbhd",
        "/model/map/t50_38_00_00_low.tpfbdt",
        "./model//obj/o00_0001.bnd",
        "model_hq/map//g50_38_00_00.gibdt",
        "model_hq/map//g50_38_00_00.gibhd",
        "model_hq/map//t10_02_00_00.tpfbdt",
        "model_hq/map/t10_02_00_00.tpfbhd",
        "model_hq///obj/o00_2000.bnd",
        "model_hq//././chr//c5000.texbnd",
        "model_hq//parts//shield//sd_1000_m.bnd",
        "model_hq/parts/shield//sd_1000_m_l.bnd",
    ];

    #[test]
    fn components_eq() {
        components(|path_as_components, components_as_components| {
            assert_eq!(path_as_components, path_as_components);
            assert_eq!(path_as_components, components_as_components);
            assert_eq!(components_as_components, path_as_components);
            assert_eq!(components_as_components, components_as_components);
        });
    }

    #[test]
    fn components_partial_ord_eq() {
        components(|path_as_components, components_as_components| {
            const EQ: Option<Ordering> = Some(Ordering::Equal);
            assert_eq!(path_as_components.partial_cmp(path_as_components), EQ);
            assert_eq!(components_as_components.partial_cmp(path_as_components), EQ);
            assert_eq!(path_as_components.partial_cmp(components_as_components), EQ);
            assert_eq!(
                components_as_components.partial_cmp(components_as_components),
                EQ
            );
        });
    }

    #[test]
    fn components_hash_eq() {
        components(|path_as_components, components_as_components| {
            let hasher = FxBuildHasher::new();
            assert_eq!(
                hasher.hash_one(path_as_components),
                hasher.hash_one(components_as_components)
            );
        });
    }

    #[test]
    fn components_partial_ord() {
        components(|path_as_components1, components_as_components1| {
            components(|path_as_components2, components_as_components2| {
                let cmp0 = path_as_components1.partial_cmp(path_as_components2);
                let cmp1 = components_as_components1.partial_cmp(path_as_components2);
                let cmp2 = path_as_components1.partial_cmp(components_as_components2);
                let cmp3 = components_as_components1.partial_cmp(components_as_components2);

                assert_eq!(cmp0, cmp1);
                assert_eq!(cmp1, cmp2);
                assert_eq!(cmp2, cmp3);
            });
        });
    }

    #[test]
    fn components_ord() {
        components(|path_as_components1, components_as_components1| {
            components(|path_as_components2, components_as_components2| {
                let cmp0 = path_as_components1.cmp(path_as_components2);
                let cmp1 = components_as_components1.cmp(path_as_components2);
                let cmp2 = path_as_components1.cmp(components_as_components2);
                let cmp3 = components_as_components1.cmp(components_as_components2);

                assert_eq!(cmp0, cmp1);
                assert_eq!(cmp1, cmp2);
                assert_eq!(cmp2, cmp3);
            });
        });
    }

    #[track_caller]
    fn components<F: Fn(&Components<&str>, &Components<&str>)>(f: F) {
        for path in PATHS {
            let path = path.to_string();
            let path_upper = path.to_ascii_uppercase();

            let components = path.split(NormalizeAscii::SEPARATORS).collect::<Vec<_>>();
            let components_upper = path_upper
                .split(NormalizeAscii::SEPARATORS)
                .collect::<Vec<_>>();

            for (path, components) in [
                (path.as_str(), components.as_slice()),
                (path_upper.as_str(), components.as_slice()),
                (path.as_str(), components_upper.as_slice()),
                (path_upper.as_str(), components_upper.as_slice()),
            ] {
                let path_as_components = path.as_components::<NormalizeAscii>();
                let components_as_components = components.as_components::<NormalizeAscii>();

                f(path_as_components, components_as_components);
            }
        }
    }
}
