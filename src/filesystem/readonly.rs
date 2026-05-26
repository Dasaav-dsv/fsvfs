use std::{
    borrow::Cow,
    collections::VecDeque,
    fmt,
    marker::PhantomData,
    mem,
    num::NonZero,
    ops::{Bound, Range, RangeBounds},
};

use fxhash::FxHashMap;
use smallvec::{SmallVec, smallvec_inline};
use thiserror::Error;

use crate::filesystem::paths::Paths;

#[derive(Debug, Error)]
pub enum RofsError {
    #[error("no such entity")]
    NotFound,

    #[error("not a file")]
    IsDir,
}

#[derive(Debug)]
pub struct RofsBuilder<'a, 'b, T> {
    files: Vec<(&'a str, T)>,
    hard_links: Vec<(&'a str, &'b str)>,
}

#[cfg_attr(feature = "rkyv", derive(rkyv::Archive, rkyv::Serialize))]
pub struct Rofs<'a, T, C: Config = DefaultConfig> {
    nodes: Box<[Node]>,
    files: Box<[T]>,
    paths: Paths<'a>,
    _config: PhantomData<C>,
}

#[derive(Debug)]
pub enum Entry<'a, T> {
    Dir(Range<u32>),
    File(&'a T),
}

#[derive(Clone, Copy, Debug)]
pub struct DefaultConfig;

pub trait Config {
    const SEPARATORS: &[char] = &['/'];

    #[inline]
    fn normalize_component(component: &str) -> Cow<'_, str> {
        Cow::Borrowed(component)
    }
}

impl Config for DefaultConfig {}

pub trait ReadOnlyFilesystem {
    type File;

    fn entry(&self, inode: u32) -> Result<Entry<'_, Self::File>, RofsError>;

    fn entries_iter<R>(
        &self,
        range: R,
    ) -> Result<impl Iterator<Item = Entry<'_, Self::File>>, RofsError>
    where
        R: RangeBounds<u32>;

    fn file_data(&self, entry: &Entry<'_, Self::File>) -> Result<u32, RofsError>;

    fn file_data_iter(&self) -> impl Iterator<Item = &Self::File>;

    fn path(&self, inode: u32) -> Result<&str, RofsError>;

    fn lookup(&self, path: &str) -> Result<u32, RofsError>;

    fn is_dir(&self, inode: u32) -> Result<bool, RofsError> {
        Ok(matches!(self.entry(inode)?, Entry::Dir(_)))
    }

    fn is_file(&self, inode: u32) -> Result<bool, RofsError> {
        Ok(matches!(self.entry(inode)?, Entry::File(_)))
    }
}

#[derive(Clone, Copy, Debug)]
#[cfg_attr(feature = "rkyv", derive(rkyv::Archive, rkyv::Serialize))]
enum Node {
    Dir {
        child_index: NonZero<u32>,
        child_count: NonZero<u32>,
    },
    File(FileNode),
}

#[derive(Clone, Copy, Debug)]
#[cfg_attr(feature = "rkyv", derive(rkyv::Archive, rkyv::Serialize))]
struct FileNode {
    data_index: u32,
}

impl<'a, 'b, T> RofsBuilder<'a, 'b, T> {
    pub const fn new() -> Self {
        Self {
            files: Vec::new(),
            hard_links: Vec::new(),
        }
    }

    pub fn with_files<I>(&mut self, iter: I) -> &mut Self
    where
        I: IntoIterator<Item = (&'a str, T)>,
    {
        self.files.extend(iter);
        self
    }

    pub fn with_hard_links<I>(&mut self, iter: I) -> &mut Self
    where
        I: IntoIterator<Item = (&'a str, &'b str)>,
    {
        self.hard_links.extend(iter);
        self
    }

    pub fn finish<C: Config>(&mut self) -> Rofs<'static, T, C> {
        let Self { files, hard_links } = mem::take(self);
        Rofs::new(files, hard_links)
    }
}

impl<T, C: Config> Rofs<'_, T, C> {
    fn new(files: Vec<(&str, T)>, hard_links: Vec<(&str, &str)>) -> Rofs<'static, T, C> {
        let _ = Self::inode_from(files.len() + hard_links.len());

        let (mut components, files): (Vec<_>, Vec<_>) = files
            .into_iter()
            .zip(0..)
            .map(|((path, file), data_index)| {
                let components = normalize_components::<C>(path);
                let file_index = FileNode { data_index };
                ((components, file_index), file)
            })
            .unzip();

        components.reserve_exact(hard_links.len());

        let mut hard_links = hard_links
            .into_iter()
            .map(|(from, to)| {
                (
                    normalize_components::<C>(from),
                    normalize_components::<C>(to),
                )
            })
            .collect::<FxHashMap<_, _>>();

        for i in 0..components.len() {
            let (component, file_index) = &components[i];

            if let Some(to) = hard_links.remove(component) {
                components.push((to, *file_index));
            }
        }

        enum TreeNode<'a> {
            Branch(FxHashMap<&'a str, TreeNode<'a>>),
            Leaf(FileNode),
        }

        let mut root = FxHashMap::default();
        let mut total = 1u64;

        for (components, file_node) in &components {
            let mut node = &mut root;
            let mut iter = components.iter().peekable();

            while let Some(component) = iter.next() {
                let next = node.entry(&**component).or_insert_with(|| {
                    total += 1;

                    match iter.peek() {
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

        let root = FxHashMap::from_iter([("/", TreeNode::Branch(root))]);

        let total = Self::inode_from(total);

        let mut nodes = Vec::<Node>::with_capacity(total as usize);
        let mut paths = Vec::<(u32, SmallVec<[&str; 1]>)>::with_capacity(total as usize);

        let mut inode = 0;
        let mut child_index = NonZero::<u32>::MIN;

        let mut queue = VecDeque::from([(0, &root)]);

        while let Some((parent_index, branch)) = queue.pop_front() {
            for (component, node) in branch {
                match node {
                    TreeNode::Branch(branch) => {
                        let Some(child_count) = NonZero::new(branch.len() as u32) else {
                            continue;
                        };

                        nodes.push(Node::Dir {
                            child_index,
                            child_count,
                        });

                        child_index = child_index.checked_add(child_count.get()).unwrap();

                        let path = match paths.get(parent_index).map(|(_, parent)| &**parent) {
                            None => smallvec_inline!["/"],
                            Some(&["/"]) => smallvec_inline![*component],
                            Some(parent) => parent.iter().cloned().chain([*component]).collect(),
                        };
                        let parent_index = paths.len();

                        queue.push_back((parent_index, branch));

                        paths.push((inode, path));
                        inode += 1;
                    }
                    TreeNode::Leaf(file_node) => {
                        nodes.push(Node::File(*file_node));

                        let path = match paths[parent_index].1.as_slice() {
                            &["/"] => smallvec_inline![*component],
                            parent => parent.iter().cloned().chain([*component]).collect(),
                        };

                        paths.push((inode, path));
                        inode += 1;
                    }
                }
            }
        }

        let paths = paths
            .iter()
            .map(|(i, p)| (*i, p.join("/")))
            .collect::<Vec<_>>();

        let paths_iter = paths.iter().map(|(i, p)| (*i, p.as_str()));

        Rofs {
            nodes: nodes.into_boxed_slice(),
            files: files.into_boxed_slice(),
            paths: Paths::from_iter(paths_iter),
            _config: PhantomData,
        }
    }

    #[track_caller]
    fn inode_from<N>(n: N) -> u32
    where
        N: TryInto<u32> + fmt::Display + Copy,
    {
        if let Ok(n) = n.try_into() {
            return n;
        }

        panic!("inode conversion failed: input ({n}) does not fit in a u32!");
    }

    fn node_to_entry(&self, node: &Node) -> Entry<'_, T> {
        match node {
            Node::Dir {
                child_index,
                child_count,
            } => {
                let start = child_index.get();
                let end = start + child_count.get();
                Entry::Dir(start..end)
            }
            Node::File(FileNode { data_index }) => Entry::File(&self.files[*data_index as usize]),
        }
    }
}

#[cfg(feature = "rkyv")]
impl<T, C: Config> ArchivedRofs<'_, T, C>
where
    T: rkyv::Archive,
{
    fn node_to_entry(&self, node: &ArchivedNode) -> Entry<'_, T::Archived> {
        match node {
            ArchivedNode::Dir {
                child_index,
                child_count,
            } => {
                let start = child_index.get();
                let end = start + child_count.get();
                Entry::Dir(start..end)
            }
            ArchivedNode::File(ArchivedFileNode { data_index }) => {
                Entry::File(&self.files.get()[data_index.to_native() as usize])
            }
        }
    }
}

impl<T, C: Config> ReadOnlyFilesystem for Rofs<'_, T, C> {
    type File = T;

    #[inline]
    fn entry(&self, inode: u32) -> Result<Entry<'_, Self::File>, RofsError> {
        let index = usize::try_from(inode).expect("index too large");
        let node = self.nodes.get(index).ok_or(RofsError::NotFound)?;
        Ok(self.node_to_entry(node))
    }

    #[inline]
    fn entries_iter<R>(
        &self,
        range: R,
    ) -> Result<impl Iterator<Item = Entry<'_, Self::File>>, RofsError>
    where
        R: RangeBounds<u32>,
    {
        let range = map_range(range);
        let nodes = self.nodes.get(range).ok_or(RofsError::NotFound)?;
        Ok(nodes.iter().map(|node| self.node_to_entry(node)))
    }

    #[inline]
    fn file_data(&self, entry: &Entry<'_, Self::File>) -> Result<u32, RofsError> {
        match entry {
            Entry::File(data) => {
                let index = self
                    .files
                    .element_offset(*data)
                    .expect("entry does not belong to this filesystem");

                Ok(index as u32)
            }
            Entry::Dir(_) => Err(RofsError::IsDir),
        }
    }

    #[inline]
    fn file_data_iter(&self) -> impl Iterator<Item = &Self::File> {
        self.files.iter()
    }

    #[inline]
    fn path(&self, inode: u32) -> Result<&str, RofsError> {
        self.paths.path_by_inode(inode).ok_or(RofsError::NotFound)
    }

    #[inline]
    fn lookup(&self, path: &str) -> Result<u32, RofsError> {
        let path = normalize_path::<C>(path);
        self.paths.inode_by_path(&path).ok_or(RofsError::NotFound)
    }
}

#[cfg(feature = "rkyv")]
impl<T, C: Config> ReadOnlyFilesystem for ArchivedRofs<'_, T, C>
where
    T: rkyv::Archive,
{
    type File = T::Archived;

    #[inline]
    fn entry(&self, inode: u32) -> Result<Entry<'_, Self::File>, RofsError> {
        let index = usize::try_from(inode).expect("index too large");
        let node = (*self.nodes).get(index).ok_or(RofsError::NotFound)?;
        Ok(self.node_to_entry(node))
    }

    #[inline]
    fn entries_iter<R>(
        &self,
        range: R,
    ) -> Result<impl Iterator<Item = Entry<'_, Self::File>>, RofsError>
    where
        R: RangeBounds<u32>,
    {
        let range = map_range(range);
        let nodes = (*self.nodes).get(range).ok_or(RofsError::NotFound)?;
        Ok(nodes.iter().map(|node| self.node_to_entry(node)))
    }

    #[inline]
    fn file_data(&self, entry: &Entry<'_, Self::File>) -> Result<u32, RofsError> {
        match entry {
            Entry::File(data) => {
                let index = self
                    .files
                    .element_offset(*data)
                    .expect("entry does not belong to this filesystem");

                Ok(index as u32)
            }
            Entry::Dir(_) => Err(RofsError::IsDir),
        }
    }

    #[inline]
    fn file_data_iter(&self) -> impl Iterator<Item = &Self::File> {
        self.files.iter()
    }

    #[inline]
    fn path(&self, inode: u32) -> Result<&str, RofsError> {
        self.paths.path_by_inode(inode).ok_or(RofsError::NotFound)
    }

    #[inline]
    fn lookup(&self, path: &str) -> Result<u32, RofsError> {
        let path = normalize_path::<C>(path);
        self.paths.inode_by_path(&path).ok_or(RofsError::NotFound)
    }
}

#[inline]
fn normalize_components<C: Config>(path: &str) -> SmallVec<[Cow<'_, str>; 4]> {
    let mut components = SmallVec::<[Cow<str>; _]>::new();

    for component in path.split(C::SEPARATORS) {
        match component {
            "" | "." => {}
            ".." => {
                if components.last().is_some_and(|parent| parent != "..") {
                    components.pop();
                }
            }
            _ => {
                components.push(C::normalize_component(component));
            }
        }
    }

    components
}

#[inline]
fn normalize_path<C: Config>(path: &str) -> Cow<'_, str> {
    if path == "/" {
        return Cow::Borrowed("/");
    }

    let mut components = normalize_components::<C>(path);
    match components.as_mut_slice() {
        [one] => mem::take(one),
        components => Cow::Owned(components.join("/")),
    }
}

#[track_caller]
fn map_range<R>(range: R) -> (Bound<usize>, Bound<usize>)
where
    R: RangeBounds<u32>,
{
    let start = range
        .start_bound()
        .map(|start| usize::try_from(*start).expect("index too large"));

    let end = range
        .end_bound()
        .map(|end| usize::try_from(*end).expect("index too large"));

    (start, end)
}

impl<T> Default for RofsBuilder<'_, '_, T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: fmt::Debug, C: Config> fmt::Debug for Rofs<'_, T, C> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Rofs")
            .field("nodes", &self.nodes)
            .field("files", &self.files)
            .field("paths", &self.paths)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use std::{borrow::Cow, fs, path::Path, sync::LazyLock};

    use crate::{
        cow::CowExt,
        filesystem::readonly::{Config, Entry, ReadOnlyFilesystem, Rofs, RofsBuilder},
        hash::hash_path32,
    };

    const PATHS: [&str; 22] = [
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
        ".gamedataebl/d0/d0d8f66f",
        ".gamedataebl/c9/c97bf2b8",
        ".gamedataebl/76/764987ae",
        ".hqmapebl/46/4676b068",
        ".hqmapebl/46/4676b0ec",
        ".hqmapebl/d0/d00e74ce",
        ".hqmapebl/d0/d00e7552",
        ".hqobjebl/47/4799c947",
        ".hqchrebl/d9/d969da6a",
        ".hqpartsebl/43/43aae981",
        ".hqpartsebl/a8/a86e5d60",
    ];

    #[test]
    fn lookup() {
        lookup_in_fs(bnd_fs(), &PATHS);
    }

    #[test]
    fn get_data() {
        get_data_in_fs(bnd_fs(), &PATHS);
    }

    #[track_caller]
    fn lookup_in_fs<F: ReadOnlyFilesystem>(f: &F, paths: &[&str])
    where
        F: ReadOnlyFilesystem,
    {
        assert_eq!(f.lookup("/").unwrap(), 0);
        assert_eq!(f.path(0).unwrap(), "/");

        for &path in paths {
            let inode = f.lookup(path).unwrap();
            assert_eq!(path, f.path(inode).unwrap());
        }
    }

    #[track_caller]
    fn get_data_in_fs<F: ReadOnlyFilesystem>(f: &F, paths: &[&str])
    where
        F::File: IntoIterator<Item: PartialEq<u32>> + Copy,
    {
        for &path in paths {
            let inode = f.lookup(path).unwrap();
            let Entry::File(&hashes) = f.entry(inode).unwrap() else {
                panic!("not a file");
            };

            let expected = hash_path32(path).unwrap();
            assert!(hashes.into_iter().any(|hash| hash == expected), "{path}",);
        }
    }

    #[derive(Debug)]
    struct BndConfig;
    impl Config for BndConfig {
        const SEPARATORS: &[char] = &['/', '\\'];

        fn normalize_component(component: &str) -> Cow<'_, str> {
            let mut component = Cow::Borrowed(component);
            Cow::make_ascii_lowercase(&mut component);
            component
        }
    }

    type BndFs<'a> = Rofs<'a, [u32; 2], BndConfig>;

    #[track_caller]
    fn bnd_fs() -> &'static BndFs<'static> {
        static FS: LazyLock<BndFs> = LazyLock::new(|| {
            let files = [
                "dist/dvdbnd/Hash/DarkSouls2_PC/GameDataEbl.txt",
                "dist/dvdbnd/Hash/DarkSouls2_PC/HqChrEbl.txt",
                "dist/dvdbnd/Hash/DarkSouls2_PC/HqMapEbl.txt",
                "dist/dvdbnd/Hash/DarkSouls2_PC/HqObjEbl.txt",
                "dist/dvdbnd/Hash/DarkSouls2_PC/HqPartsEbl.txt",
            ]
            .into_iter()
            .map(|path| (Path::new(path), fs::read_to_string(path).unwrap()))
            .collect::<Vec<_>>();

            let path_hashes = files
                .iter()
                .flat_map(|(bnd_path, file)| {
                    file.lines().map(|path| {
                        let hash = hash_path32(path).unwrap();
                        let bnd = bnd_path.file_prefix().unwrap().to_str().unwrap();
                        let hash_path = format!(".{bnd}/{:02x}/{hash:08x}", hash >> 24);
                        let hash_path_hash = hash_path32(&hash_path).unwrap();
                        (path, hash_path, [hash, hash_path_hash])
                    })
                })
                .collect::<Vec<_>>();

            RofsBuilder::new()
                .with_files(
                    path_hashes
                        .iter()
                        .map(|(_, hash_path, hashes)| (hash_path.as_str(), *hashes)),
                )
                .with_hard_links(path_hashes.iter().map(|(to, from, _)| (from.as_str(), *to)))
                .finish()
        });

        &FS
    }

    #[cfg(feature = "rkyv")]
    mod rkyv_tests {
        use rkyv::{rancor::Error, util::AlignedVec};

        use crate::filesystem::readonly::ArchivedRofs;

        use super::*;

        #[test]
        fn lookup() {
            lookup_in_fs(archived_bnd_fs(), &PATHS);
        }

        #[test]
        fn get_data() {
            get_data_in_fs(archived_bnd_fs(), &PATHS);
        }

        type ArchivedBndFs<'a> = ArchivedRofs<'a, [u32; 2], BndConfig>;

        #[track_caller]
        fn archived_bnd_fs() -> &'static ArchivedBndFs<'static> {
            static FS_BYTES: LazyLock<AlignedVec> = LazyLock::new(|| {
                let fs = bnd_fs();
                rkyv::to_bytes::<Error>(fs).unwrap()
            });

            static ARHIVED_FS: LazyLock<&'static ArchivedBndFs<'static>> =
                LazyLock::new(|| rkyv::access::<_, Error>(&FS_BYTES).unwrap());

            &ARHIVED_FS
        }
    }
}
