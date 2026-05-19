use std::{collections::HashMap, fmt, ptr::NonNull};

cfg_select! {
    feature = "rkyv" => {
        mod rkyv;
        pub use rkyv::*;
    }
    _ => {
        use fxhash::FxHashMap;

        pub struct Paths<'a> {
            inner: RawPaths<'a>,
        }

        struct RawPaths<'a> {
            paths_by_inode: FxHashMap<u32, &'a str>,
            inodes_by_path: FxHashMap<&'a str, u32>,
            str_store: Option<NonNull<str>>,
        }
    }
}

impl Paths<'_> {
    pub fn path_by_inode(&self, inode: u32) -> Option<&str> {
        self.reborrow().paths_by_inode.get(&inode).cloned()
    }

    pub fn inode_by_path(&self, path: &str) -> Option<u32> {
        self.reborrow().inodes_by_path.get(&path).cloned()
    }

    fn reborrow<'a>(&'a self) -> &'a RawPaths<'a> {
        &self.inner
    }
}

impl<'a> FromIterator<(u32, &'a str)> for Paths<'static> {
    fn from_iter<T: IntoIterator<Item = (u32, &'a str)>>(iter: T) -> Self {
        let mut pos = 0;
        let (kv, str_store) = iter
            .into_iter()
            .map(|(inode, path)| {
                let start = pos;
                pos += path.len();
                ((inode, start..pos), path)
            })
            .unzip::<_, _, Vec<_>, String>();

        let str_store = NonNull::new(Box::into_raw(str_store.into_boxed_str())).unwrap();

        let (mut paths_by_inode, mut inodes_by_path) = (
            HashMap::with_capacity_and_hasher(kv.len(), Default::default()),
            HashMap::with_capacity_and_hasher(kv.len(), Default::default()),
        );

        for (inode, str_range) in kv {
            // SAFETY: materialized 'static references do not escape.
            // `str_range` represents a valid UTF-8 range.
            let path = paths_by_inode
                .entry(inode)
                .or_insert_with(|| unsafe { str_store.as_ref().get_unchecked(str_range) });

            inodes_by_path.insert(*path, inode);
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

impl Drop for Paths<'_> {
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

impl fmt::Debug for Paths<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list()
            .entries(self.reborrow().inodes_by_path.iter())
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

        assert_eq!(paths.inode_by_path("c"), Some(2));
        assert_eq!(paths.inode_by_path("d"), Some(3));
        assert_eq!(paths.inode_by_path("e"), None);
    }

    fn build_paths() -> Paths<'static> {
        [(0, "a"), (1, "b"), (2, "c"), (3, "d")]
            .into_iter()
            .collect()
    }
}
