use std::ptr::NonNull;

use fxhash::FxHashMap;
use rkyv::{
    Archive, Serialize,
    with::{Identity, InlineAsBox, MapKV, Skip},
};

#[derive(Archive, Serialize)]
pub struct Paths<'a> {
    pub(super) inner: RawPaths<'a>,
}

#[derive(Archive, Serialize)]
pub(super) struct RawPaths<'a> {
    #[rkyv(with = MapKV<Identity, InlineAsBox>)]
    pub(super) paths_by_inode: FxHashMap<u32, &'a str>,

    #[rkyv(with = MapKV<InlineAsBox, Identity>)]
    pub(super) inodes_by_path: FxHashMap<&'a str, u32>,

    #[rkyv(with = Skip)]
    pub(super) str_store: Option<NonNull<str>>,
}

impl ArchivedPaths<'_> {
    pub fn path_by_inode(&self, inode: u32) -> Option<&str> {
        let boxed = self.inner.paths_by_inode.get(&inode.into())?;
        Some(&**boxed)
    }

    pub fn inode_by_path(&self, path: &str) -> Option<u32> {
        let inode = self.inner.inodes_by_path.get(path)?;
        Some(inode.into())
    }
}
