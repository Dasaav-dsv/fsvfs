use std::{
    borrow::Cow,
    collections::VecDeque,
    fmt,
    hint::cold_path,
    iter::Peekable,
    marker::PhantomData,
    mem,
    num::NonZero,
    ops::{Bound, Range, RangeBounds},
};

use eytzinger::{SliceExt, permutation::InplacePermutator};
use fxhash::FxHashMap;
use rkyv::{Archive, Serialize};
use thiserror::Error;

use crate::cow::CowExt;

#[derive(Debug, Error)]
pub enum RofsError {
    #[error("no such entity")]
    NotFound,

    #[error("not a file")]
    IsDir,
}

#[derive(Debug)]
pub struct RofsBuilder<'a, T> {
    files: Vec<(&'a str, T)>,
}

#[derive(Archive, Serialize)]
pub struct Rofs<T, C: Config = DefaultConfig> {
    nodes: Box<[Node]>,
    files: Box<[T]>,
    names: Box<[u8]>,
    _config: PhantomData<C>,
}

#[derive(Debug)]
pub enum Entry<'a, T> {
    Dir(Range<u64>),
    File(&'a T),
}

#[derive(Clone, Copy, Debug)]
pub struct DefaultConfig;

pub trait Config {
    const INODE_ROOT: u64 = 1;
    const SEPARATORS: &[char] = &['/'];
}

impl Config for DefaultConfig {}

pub trait ReadOnlyFilesystem {
    type File;

    fn name(&self, inode: u64) -> Result<&str, RofsError>;

    fn entry(&self, inode: u64) -> Result<Entry<'_, Self::File>, RofsError>;

    #[cfg_attr(all(unix, not(test)), expect(unused))]
    fn lookup(&self, path: &str) -> Result<u64, RofsError>;

    fn lookup_in_dir(&self, parent_inode: u64, path: &str) -> Result<u64, RofsError>;

    #[cfg_attr(any(windows, not(test)), expect(unused))]
    fn entry_iter<R>(
        &self,
        range: R,
    ) -> Result<impl ExactSizeIterator<Item = Entry<'_, Self::File>>, RofsError>
    where
        R: RangeBounds<u64>;
}

#[derive(Clone, Copy, Debug, Archive, Serialize)]
struct Node {
    content: NodeContent,
    name_index: u32,
}

#[derive(Clone, Copy, Debug, Archive, Serialize)]
enum NodeContent {
    Dir {
        child_index: NonZero<u32>,
        child_count: u32,
    },
    File {
        data_index: u32,
    },
}

impl<'a, T> RofsBuilder<'a, T> {
    pub const fn new() -> Self {
        Self { files: Vec::new() }
    }

    pub fn with_files<I>(&mut self, iter: I) -> &mut Self
    where
        I: IntoIterator<Item = (&'a str, T)>,
    {
        self.files.extend(iter);
        self
    }

    pub fn finish<C: Config>(&mut self) -> Rofs<T, C> {
        let Self { files } = mem::take(self);
        Rofs::new(files)
    }
}

impl<T, C: Config> Rofs<T, C> {
    fn new(files: Vec<(&str, T)>) -> Rofs<T, C> {
        assert_u32(files.len());

        let (file_paths, files): (Vec<_>, Vec<_>) = files
            .into_iter()
            .zip(0..)
            .map(|((path, file), data_index)| {
                let mut path = normalize_path::<C>(path);
                Cow::to_ascii_lowercase(&mut path);
                ((path, data_index), file)
            })
            .unzip();

        let files = files.into_boxed_slice();

        enum TreeNode<'a> {
            Branch(FxHashMap<&'a str, TreeNode<'a>>),
            Leaf(u32),
        }

        let mut root = FxHashMap::default();

        let mut total = 1usize;
        let mut total_len = 0usize;

        for (file_path, file_node) in &file_paths {
            let mut node = &mut root;

            let mut components = components::<C>(file_path);

            while let Some(component) = components.next() {
                assert!(
                    component.len() < u8::MAX as usize,
                    "file names must be shorter than 255 bytes ({component})",
                );

                total_len += component.len() + 1;

                let next = node.entry(component).or_insert_with(|| {
                    total += 1;

                    match components.peek() {
                        Some(_) => TreeNode::Branch(Default::default()),
                        None => TreeNode::Leaf(*file_node),
                    }
                });

                match next {
                    TreeNode::Branch(next) => node = next,
                    TreeNode::Leaf(_) => break,
                }
            }
        }

        assert_u32(total);

        let root = FxHashMap::from_iter([(".", TreeNode::Branch(root))]);

        let mut nodes = Vec::<Node>::with_capacity(total);

        let mut names = Vec::with_capacity(total_len);
        let mut names_interned = FxHashMap::with_capacity_and_hasher(total, Default::default());

        let mut queue = VecDeque::from([&root]);
        let mut child_index = NonZero::<u32>::MIN;

        while let Some(branch) = queue.pop_front() {
            let first = nodes.len();

            for (&component, node) in branch {
                let name_index = *names_interned.entry(component).or_insert_with(|| {
                    let index = names.len();
                    names.push(component.len() as u8);
                    names.extend_from_slice(component.as_bytes());
                    index
                });

                match node {
                    TreeNode::Branch(branch) => {
                        let child_count = branch.len() as u32;

                        nodes.push(Node::new_dir(child_index, child_count, name_index));
                        child_index = child_index.checked_add(child_count).unwrap();

                        queue.push_back(branch);
                    }
                    TreeNode::Leaf(data_index) => {
                        nodes.push(Node::new_file(*data_index, name_index));
                    }
                }
            }

            // SAFETY: indices and lengths are valid for length-prefixed strings.
            nodes[first..].sort_unstable_by_key(|node| unsafe {
                let name = names.get_unchecked(node.name_index()..);
                let (len, rest) = name.split_first().unwrap_unchecked();
                rest.get_unchecked(..*len as usize)
            });

            nodes[first..].eytzingerize(&mut InplacePermutator);
        }

        Rofs {
            nodes: nodes.into_boxed_slice(),
            names: names.into_boxed_slice(),
            files,
            _config: PhantomData,
        }
    }

    #[inline]
    fn node_to_entry(&self, node: &Node) -> Entry<'_, T> {
        match node.content {
            NodeContent::Dir {
                child_index,
                child_count,
            } => {
                let start = (child_index.get() as u64).wrapping_add(C::INODE_ROOT);
                let end = start.wrapping_add(child_count as u64);
                Entry::Dir(start..end)
            }
            NodeContent::File { data_index } => {
                let data_index = data_index as usize;
                Entry::File(&self.files[data_index])
            }
        }
    }

    #[inline]
    fn inode_by_path_ignore_ascii_case(&self, parent: u32, mut path: Cow<'_, str>) -> Option<u32> {
        match self.inode_by_path(parent, &path) {
            Some(inode) => Some(inode),
            None => {
                Cow::to_ascii_lowercase(&mut path).then(|| self.inode_by_path(parent, &path))?
            }
        }
    }

    #[inline]
    fn inode_by_path(&self, parent: u32, path: &str) -> Option<u32> {
        let mut components = components::<C>(path);

        if components.peek().is_none() {
            return Some(parent);
        }

        let mut index = parent as usize;

        loop {
            let component = components.next()?;

            let &Node {
                content:
                    NodeContent::Dir {
                        child_index,
                        child_count,
                    },
                ..
            } = self.nodes.get(index)?
            else {
                break None;
            };

            let start = child_index.get() as usize;
            let end = start + child_count as usize;

            // SAFETY: same as in `Rofs::new`.
            // Note this wouldn't be safe in the archived version.
            index = start
                + self.nodes[start..end].eytzinger_search_by_key(
                    &component.as_bytes(),
                    |node| unsafe {
                        let name = self.names.get_unchecked(node.name_index()..);
                        let (len, rest) = name.split_first().unwrap_unchecked();
                        rest.get_unchecked(..*len as usize)
                    },
                )?;

            if components.peek().is_none() {
                break Some(index as u32);
            }
        }
    }
}

#[track_caller]
fn assert_u32<N>(n: N) -> u32
where
    N: TryInto<u32> + fmt::Display + Copy,
{
    if let Ok(n) = n.try_into() {
        return n;
    }

    panic!("conversion failed: input ({n}) does not fit in a u32!");
}

impl Node {
    fn new_dir(child_index: NonZero<u32>, child_count: u32, name_index: usize) -> Self {
        let name_index = u32::try_from(name_index).expect("name index too large");
        Self {
            content: NodeContent::Dir {
                child_index,
                child_count,
            },
            name_index,
        }
    }

    fn new_file(data_index: u32, name_index: usize) -> Self {
        let name_index = u32::try_from(name_index).expect("name index too large");
        Self {
            content: NodeContent::File { data_index },
            name_index,
        }
    }

    fn name_index(&self) -> usize {
        self.name_index as usize
    }
}

fn components<C: Config>(path: &str) -> Peekable<impl Iterator<Item = &str>> {
    path.split(C::SEPARATORS)
        .filter(|c| !matches!(*c, "" | "."))
        .peekable()
}

impl<T, C: Config> ArchivedRofs<T, C>
where
    T: rkyv::Archive,
{
    #[inline]
    fn node_to_entry(&self, node: &ArchivedNode) -> Entry<'_, T::Archived> {
        match node.content {
            ArchivedNodeContent::Dir {
                child_index,
                child_count,
            } => {
                let start = (child_index.get() as u64).wrapping_add(C::INODE_ROOT);
                let end = start.wrapping_add(child_count.to_native() as u64);
                Entry::Dir(start..end)
            }
            ArchivedNodeContent::File { data_index } => {
                let data_index = data_index.to_native() as usize;
                Entry::File(&self.files[data_index])
            }
        }
    }

    #[inline]
    fn inode_by_path_ignore_ascii_case(&self, parent: u32, mut path: Cow<'_, str>) -> Option<u32> {
        match self.inode_by_path(parent, &path) {
            Some(inode) => Some(inode),
            None => {
                Cow::to_ascii_lowercase(&mut path).then(|| self.inode_by_path(parent, &path))?
            }
        }
    }

    #[inline]
    fn inode_by_path(&self, parent: u32, path: &str) -> Option<u32> {
        let mut components = components::<C>(path);

        if components.peek().is_none() {
            return Some(parent);
        }

        let mut index = parent as usize;

        loop {
            let component = components.next()?;

            let &ArchivedNode {
                content:
                    ArchivedNodeContent::Dir {
                        child_index,
                        child_count,
                    },
                ..
            } = (*self.nodes).get(index)?
            else {
                break None;
            };

            let start = child_index.get() as usize;
            let end = start + child_count.to_native() as usize;

            index = start
                + self.nodes[start..end].eytzinger_search_by_key(
                    &component.as_bytes(),
                    |node| {
                        let name_index = node.name_index.to_native() as usize;
                        let name = &self.names[name_index..];
                        let (len, rest) = name.split_first().unwrap();
                        &rest[..*len as usize]
                    },
                )?;

            if components.peek().is_none() {
                break Some(index as u32);
            }
        }
    }
}

impl<T, C: Config> ReadOnlyFilesystem for Rofs<T, C> {
    type File = T;

    #[inline]
    fn name(&self, inode: u64) -> Result<&str, RofsError> {
        let index = inode.wrapping_sub(C::INODE_ROOT) as usize;
        let node = self.nodes.get(index).ok_or(RofsError::NotFound)?;

        // SAFETY: same as in `Rofs::new`.
        // Note this wouldn't be safe in the archived version.
        unsafe {
            let name = self.names.get_unchecked(node.name_index()..);
            let (len, rest) = name.split_first().unwrap_unchecked();
            let bytes = rest.get_unchecked(..*len as usize);

            Ok(str::from_utf8_unchecked(bytes))
        }
    }

    #[inline]
    fn entry(&self, inode: u64) -> Result<Entry<'_, Self::File>, RofsError> {
        let index = inode.wrapping_sub(C::INODE_ROOT) as usize;
        let node = self.nodes.get(index).ok_or(RofsError::NotFound)?;
        Ok(self.node_to_entry(node))
    }

    #[inline]
    fn lookup(&self, path: &str) -> Result<u64, RofsError> {
        self.lookup_in_dir(C::INODE_ROOT, path)
    }

    #[inline]
    fn lookup_in_dir(&self, parent_inode: u64, path: &str) -> Result<u64, RofsError> {
        let parent = parent_inode.wrapping_sub(C::INODE_ROOT) as u32;
        let path = normalize_path::<C>(path);

        match self.inode_by_path_ignore_ascii_case(parent, path) {
            Some(inode) => Ok((inode as u64).wrapping_add(C::INODE_ROOT)),
            None => Err(RofsError::NotFound),
        }
    }

    #[inline]
    fn entry_iter<R>(
        &self,
        range: R,
    ) -> Result<impl ExactSizeIterator<Item = Entry<'_, Self::File>>, RofsError>
    where
        R: RangeBounds<u64>,
    {
        let range = map_range(range, C::INODE_ROOT);
        let nodes = self.nodes.get(range).ok_or(RofsError::NotFound)?;
        Ok(nodes.iter().map(|node| self.node_to_entry(node)))
    }
}

impl<T, C: Config> ReadOnlyFilesystem for ArchivedRofs<T, C>
where
    T: rkyv::Archive,
{
    type File = T::Archived;

    #[inline]
    fn name(&self, inode: u64) -> Result<&str, RofsError> {
        let index = inode.wrapping_sub(C::INODE_ROOT) as usize;
        let node = (*self.nodes).get(index).ok_or(RofsError::NotFound)?;
        let name_index = node.name_index.to_native() as usize;

        let name = &self.names[name_index..];
        let (len, rest) = name.split_first().unwrap();
        let bytes = &rest[..*len as usize];

        Ok(str::from_utf8(bytes).unwrap())
    }

    #[inline]
    fn entry(&self, inode: u64) -> Result<Entry<'_, Self::File>, RofsError> {
        let index = inode.wrapping_sub(C::INODE_ROOT) as usize;
        let node = (*self.nodes).get(index).ok_or(RofsError::NotFound)?;
        Ok(self.node_to_entry(node))
    }

    #[inline]
    fn lookup(&self, path: &str) -> Result<u64, RofsError> {
        self.lookup_in_dir(C::INODE_ROOT, path)
    }

    #[inline]
    fn lookup_in_dir(&self, parent_inode: u64, path: &str) -> Result<u64, RofsError> {
        let parent = parent_inode.wrapping_sub(C::INODE_ROOT) as u32;
        let path = normalize_path::<C>(path);

        match self.inode_by_path_ignore_ascii_case(parent, path) {
            Some(inode) => Ok((inode as u64).wrapping_add(C::INODE_ROOT)),
            None => Err(RofsError::NotFound),
        }
    }

    #[inline]
    fn entry_iter<R>(
        &self,
        range: R,
    ) -> Result<impl ExactSizeIterator<Item = Entry<'_, Self::File>>, RofsError>
    where
        R: RangeBounds<u64>,
    {
        let range = map_range(range, C::INODE_ROOT);
        let nodes = (*self.nodes).get(range).ok_or(RofsError::NotFound)?;
        Ok(nodes.iter().map(|node| self.node_to_entry(node)))
    }
}

#[inline]
fn normalize_path<C: Config>(path: &str) -> Cow<'_, str> {
    if path.is_empty() {
        return Cow::Borrowed(".");
    }

    if !path.contains("..") {
        Cow::Borrowed(path)
    } else {
        cold_path();

        let mut components = vec![];

        for component in path.split(C::SEPARATORS) {
            match component {
                "" | "." => {}
                ".." => {
                    components.pop();
                }
                _ => {
                    components.push(component);
                }
            }
        }

        Cow::Owned(components.join("/"))
    }
}

#[track_caller]
fn map_range<R>(range: R, root: u64) -> (Bound<usize>, Bound<usize>)
where
    R: RangeBounds<u64>,
{
    let start = range
        .start_bound()
        .map(|start| usize::try_from(start.wrapping_sub(root)).expect("index too large"));

    let end = range
        .end_bound()
        .map(|end| usize::try_from(end.wrapping_sub(root)).expect("index too large"));

    (start, end)
}

impl<T> Default for RofsBuilder<'_, T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: fmt::Debug, C: Config> fmt::Debug for Rofs<T, C> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Rofs")
            .field("nodes", &self.nodes)
            .field("files", &self.files)
            .field("paths", &self.names)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use std::{fmt, fs, sync::LazyLock};

    use crate::{
        filesystem::{Config, Entry, ReadOnlyFilesystem, Rofs, RofsBuilder},
        hash::hash_path32,
    };

    const PATHS: [&str; 11] = [
        "model/map/t50_38_00_00.tpfbhd",
        "model/map/t50_38_00_00_low.tpfbdt",
        "model/obj/o00_0001.bnd",
        "model_hq/map/g50_38_00_00.gibdt",
        "model_hq/map/g50_38_00_00.gibhd",
        "model_hq/map/t10_02_00_00.tpfbdt",
        "model_hq/map/t10_02_00_00.tpfbhd",
        "model_hq/obj/o00_2000.bnd",
        "model_hq/chr/c5000.texbnd",
        "model_hq/parts/shield/sd_1000_m.bnd",
        "model_hq/parts/shield/sd_1000_m_l.bnd",
    ];

    #[test]
    fn lookup() {
        lookup_in_fs(bnd_fs(), &PATHS);
    }

    #[test]
    fn get_data() {
        get_data_in_fs(bnd_fs(), &PATHS);
    }

    fn lookup_in_fs<F: ReadOnlyFilesystem>(f: &F, paths: &[&str])
    where
        F: ReadOnlyFilesystem,
    {
        assert_eq!(f.lookup(".").unwrap(), 1);
        assert_eq!(f.name(1).unwrap(), ".");

        for &path in paths {
            let inode = f.lookup(path).unwrap();

            let name = match path.rsplit_once('/') {
                Some((_, name)) => name,
                None => path,
            };

            let name2 = f.name(inode).unwrap();
            assert!(name.eq_ignore_ascii_case(name2), "{name} != {name2}");
        }
    }

    fn get_data_in_fs<F: ReadOnlyFilesystem>(f: &F, paths: &[&str])
    where
        F::File: PartialEq<u32> + fmt::Debug + Copy,
    {
        for &path in paths {
            let inode = f.lookup(path).unwrap();
            let Entry::File(&hash) = f.entry(inode).unwrap() else {
                panic!("not a file");
            };

            let expected = hash_path32(path).unwrap();
            assert_eq!(hash, expected, "{path}",);
        }
    }

    #[derive(Debug)]
    struct BndConfig;
    impl Config for BndConfig {
        const SEPARATORS: &[char] = &['/', '\\'];
    }

    type BndFs = Rofs<u32, BndConfig>;

    #[track_caller]
    fn bnd_fs() -> &'static BndFs {
        static FS: LazyLock<BndFs> =
            LazyLock::new(|| {
                let files = [
                    "dist/dvdbnd/Hash/DarkSouls2_PC/GameDataEbl.txt",
                    "dist/dvdbnd/Hash/DarkSouls2_PC/HqChrEbl.txt",
                    "dist/dvdbnd/Hash/DarkSouls2_PC/HqMapEbl.txt",
                    "dist/dvdbnd/Hash/DarkSouls2_PC/HqObjEbl.txt",
                    "dist/dvdbnd/Hash/DarkSouls2_PC/HqPartsEbl.txt",
                ]
                .into_iter()
                .map(|path| fs::read_to_string(path).unwrap())
                .collect::<Vec<_>>();

                RofsBuilder::new()
                    .with_files(files.iter().flat_map(|file| {
                        file.lines().map(|path| (path, hash_path32(path).unwrap()))
                    }))
                    .finish()
            });

        &FS
    }

    mod rkyv_tests {
        use rkyv::{rancor::Error, util::AlignedVec};

        use crate::filesystem::ArchivedRofs;

        use super::*;

        #[test]
        fn lookup() {
            lookup_in_fs(archived_bnd_fs(), &PATHS);
        }

        #[test]
        fn get_data() {
            get_data_in_fs(archived_bnd_fs(), &PATHS);
        }

        type ArchivedBndFs = ArchivedRofs<u32, BndConfig>;

        #[track_caller]
        fn archived_bnd_fs() -> &'static ArchivedBndFs {
            static FS_BYTES: LazyLock<AlignedVec> = LazyLock::new(|| {
                let fs = bnd_fs();
                rkyv::to_bytes::<Error>(fs).unwrap()
            });

            static ARHIVED_FS: LazyLock<&'static ArchivedBndFs> =
                LazyLock::new(|| rkyv::access::<_, Error>(&FS_BYTES).unwrap());

            &ARHIVED_FS
        }
    }
}
