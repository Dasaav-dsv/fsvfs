use std::{fmt, ptr::NonNull};

use fxhash::FxHashMap;
use hashbrown::HashMap;

use crate::filesystem::{
    paths::components::{AsComponents, ComponentStr},
    readonly::Config,
};

pub mod components;

cfg_select! {
    feature = "rkyv" => {
        mod rkyv;
        pub use rkyv::*;
    }
    _ => {
        use fxhash::FxBuildHasher;

        use crate::filesystem::readonly::DefaultConfig;

        pub struct Paths<'a, C = DefaultConfig> {
            inner: RawPaths<'a, C>,
        }

        struct RawPaths<'a, C> {
            paths_by_inode: FxHashMap<u32, &'a str>,
            inodes_by_path: HashMap<ComponentStr<'a, C>, u32, FxBuildHasher>,
            str_store: Option<NonNull<str>>,
        }
    }
}

impl<C> Paths<'_, C> {
    pub fn path_by_inode(&self, inode: u32) -> Option<&str> {
        self.reborrow().paths_by_inode.get(&inode).cloned()
    }

    pub fn inode_by_path<'a, S>(&self, path: &S) -> Option<u32>
    where
        S: AsComponents + ?Sized,
        C: Config,
    {
        self.reborrow()
            .inodes_by_path
            .get(path.as_components())
            .cloned()
    }

    fn reborrow<'a>(&'a self) -> &'a RawPaths<'a, C> {
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
        let mut kv = Vec::with_capacity(iter_hint);
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

            kv.push((inode, start..pos));
        }

        str_store.make_ascii_lowercase();

        let str_store = NonNull::new(Box::into_raw(str_store.into_boxed_str())).unwrap();

        let (mut paths_by_inode, mut inodes_by_path) = (
            FxHashMap::with_capacity_and_hasher(kv.len(), Default::default()),
            HashMap::with_capacity_and_hasher(kv.len(), Default::default()),
        );

        for (inode, str_range) in kv {
            // SAFETY: materialized 'static references do not escape.
            // `str_range` represents a valid UTF-8 range.
            let path = paths_by_inode
                .entry(inode)
                .or_insert_with(|| unsafe { str_store.as_ref().get_unchecked(str_range) });

            inodes_by_path.insert(ComponentStr::new(*path), inode);
        }

        paths_by_inode.shrink_to_fit();
        inodes_by_path.shrink_to_fit();

        Self {
            inner: RawPaths {
                paths_by_inode,
                inodes_by_path,
                str_store: Some(str_store),
            },
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
unsafe impl<C> Send for RawPaths<'_, C> {}

// SAFETY: we promise not to expose the fake 'static lifetime
// or otherwise violate memory safety.
unsafe impl<C> Sync for RawPaths<'_, C> {}

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
