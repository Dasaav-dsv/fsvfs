use std::{collections::HashMap, hash::BuildHasherDefault, ptr::NonNull};

use rkyv::{Archive, Serialize, hash::FxHasher64, with::Skip};

#[derive(Archive, Serialize)]
pub struct Paths<'a> {
    pub(super) inner: RawPaths<'a>,
}

#[derive(Archive, Serialize)]
pub(super) struct RawPaths<'a> {
    pub(super) paths_by_inode: HashMap<u32, &'a str, BuildHasherDefault<FxHasher64>>,
    pub(super) inodes_by_path: HashMap<&'a str, u32, BuildHasherDefault<FxHasher64>>,

    #[rkyv(with = Skip)]
    pub(super) str_store: Option<NonNull<str>>,
}
