use std::{fmt, hash::BuildHasherDefault, marker::PhantomData, ptr::NonNull};

use hashbrown::HashMap;
use rkyv::{
    Archive, Serialize,
    hash::FxHasher64,
    with::{Identity, InlineAsBox, Map, MapKV, Skip},
};

use crate::filesystem::{
    components::{AsComponents, ComponentStr},
    readonly::{Config, DefaultConfig, Normalize},
};

#[derive(Archive, Serialize)]
pub struct Paths<'a, C = DefaultConfig> {
    inner: RawPaths<'a>,
    _marker: PhantomData<C>,
}

#[derive(Archive, Serialize)]
struct RawPaths<'a> {
    #[rkyv(with = Map<InlineAsBox>)]
    paths_by_inode: Vec<&'a str>,

    #[rkyv(with = MapKV<InlineAsBox, Identity>)]
    inodes_by_path: HashMap<ComponentStr<'a, DefaultConfig>, u32, BuildHasherDefault<FxHasher64>>,

    #[rkyv(with = Skip)]
    str_store: Option<NonNull<str>>,
}

impl<C> ArchivedPaths<'_, C> {
    pub fn path_by_inode(&self, inode: u32) -> Option<&str> {
        let boxed = self.inner.paths_by_inode.get(inode as usize)?;
        Some(&**boxed)
    }

    pub fn inode_by_path(&self, path: &(impl AsComponents + ?Sized)) -> Option<u32>
    where
        C: Config,
    {
        let inode = self
            .inner
            .inodes_by_path
            .get_with(path.as_components::<C>(), |components, key| {
                components == key.as_components()
            })
            .cloned()?;

        Some(inode.into())
    }
}

impl<C> Paths<'_, C> {
    pub fn path_by_inode(&self, inode: u32) -> Option<&str> {
        self.reborrow().paths_by_inode.get(inode as usize).cloned()
    }

    pub fn inode_by_path(&self, path: &(impl AsComponents + ?Sized)) -> Option<u32>
    where
        C: Config,
    {
        self.reborrow()
            .inodes_by_path
            .get(path.as_components::<C>())
            .cloned()
    }

    fn reborrow<'a>(&'a self) -> &'a RawPaths<'a> {
        &self.inner
    }
}

impl<'a, S, C> FromIterator<(u32, &'a [S])> for Paths<'static, C>
where
    S: AsRef<str>,
    C: Config,
    String: Extend<S>,
{
    fn from_iter<T: IntoIterator<Item = (u32, &'a [S])>>(iter: T) -> Self {
        let iter = iter.into_iter();
        let iter_hint = iter.size_hint().0;

        let mut pos = 0;
        let mut str_ranges = vec![0..0usize; iter_hint];
        let mut str_store = String::with_capacity(iter_hint * 32);

        for (inode, path) in iter {
            let start = pos;

            let Some((file, dirs)) = path.split_last() else {
                continue;
            };

            for dir in dirs {
                let dir = dir.as_ref();
                pos += dir.len() + 1;
                str_store += dir;
                str_store.push('/');
            }

            let file = file.as_ref();
            pos += file.len();
            str_store += file;

            let index = inode as usize;
            if str_ranges.len() < index {
                str_ranges.resize(index + 1, 0..0usize);
            }

            str_ranges[index] = start..pos;
        }

        if const { matches!(C::NORMALIZATION, Normalize::AsciiCase) } {
            str_store.make_ascii_lowercase();
        }

        let str_store = NonNull::new(Box::into_raw(str_store.into_boxed_str())).unwrap();

        // SAFETY: materialized 'static references do not escape.
        // `str_range` represents a valid UTF-8 range.
        let paths_by_inode = str_ranges
            .into_iter()
            .map(|range| unsafe { str_store.as_ref().get_unchecked(range) })
            .collect::<Vec<_>>();

        let inodes_by_path = paths_by_inode
            .iter()
            .zip(0..)
            .map(|(path, inode)| (ComponentStr::<DefaultConfig>::new(*path), inode))
            .collect();

        Self {
            inner: RawPaths {
                paths_by_inode,
                inodes_by_path,
                str_store: Some(str_store),
            },
            _marker: PhantomData,
        }
    }
}

impl<C> Drop for Paths<'_, C> {
    fn drop(&mut self) {
        // No other references can outlive self:
        self.inner.paths_by_inode = Default::default();
        self.inner.inodes_by_path = Default::default();

        if let Some(str_store) = self.inner.str_store.take() {
            // SAFETY: all references to the underlying storage have been dropped.
            unsafe {
                let _ = Box::from_raw(str_store.as_ptr());
            }
        }
    }
}

impl<C> fmt::Debug for Paths<'_, C> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list()
            .entries(&self.reborrow().paths_by_inode)
            .finish()
    }
}

// SAFETY: we promise not to expose the fake 'static lifetime
// or otherwise violate memory safety.
unsafe impl Send for RawPaths<'_> {}

// SAFETY: we promise not to expose the fake 'static lifetime
// or otherwise violate memory safety.
unsafe impl Sync for RawPaths<'_> {}

#[cfg(test)]
mod tests {
    use crate::filesystem::paths::Paths;

    #[test]
    fn get_path() {
        let paths = build_paths();

        assert_eq!(paths.path_by_inode(0), Some("a"));
        assert_eq!(paths.path_by_inode(1), Some("b"));
        assert_eq!(paths.path_by_inode(4), None);
    }

    #[test]
    fn get_inode() {
        let paths = build_paths();

        assert_eq!(paths.inode_by_path(&"c"), Some(2));
        assert_eq!(paths.inode_by_path(&"d"), Some(3));
        assert_eq!(paths.inode_by_path(&"e"), None);
    }

    fn build_paths() -> Paths<'static> {
        [
            (0, ["a"].as_slice()),
            (1, ["b"].as_slice()),
            (2, ["c"].as_slice()),
            (3, ["d"].as_slice()),
        ]
        .into_iter()
        .collect()
    }
}
