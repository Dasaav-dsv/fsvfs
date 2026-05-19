use std::ptr::NonNull;

use fxhash::FxHashMap;
use rkyv::{Archive, Serialize, with::Skip};

#[derive(Archive, Serialize)]
pub struct Paths<'a> {
    pub(super) inner: RawPaths<'a>,
}

#[derive(Archive, Serialize)]
pub(super) struct RawPaths<'a> {
    pub(super) paths_by_inode: FxHashMap<u32, &'a str>,
    pub(super) inodes_by_path: FxHashMap<&'a str, u32>,

    #[rkyv(with = Skip)]
    pub(super) str_store: Option<NonNull<str>>,
}
