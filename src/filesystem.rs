use std::{borrow::Cow, fmt, hint::cold_path, iter::Peekable, marker::PhantomData, num::NonZero};

use eytzinger::SliceExt;
use thiserror::Error;

use crate::cow::CowExt;

pub mod object;

#[derive(Debug, Error)]
pub enum RofsError {
    #[error("no such entity")]
    NotFound,

    #[error("not a file")]
    IsDir,
}

pub struct Rofs<T, C: Config = DefaultConfig> {
    nodes: Box<[Node]>,
    files: Box<[T]>,
    names: Box<[u8]>,
    _config: PhantomData<C>,
}

#[derive(Debug)]
pub struct Entry<'a, T> {
    pub inode: u64,
    pub name: &'a str,
    pub kind: EntryKind<'a, T>,
}

pub enum EntryKind<'a, T> {
    Dir(Box<dyn ExactSizeIterator<Item = Entry<'a, T>> + 'a>),
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

    fn lookup(&self, path: &str) -> Result<u64, RofsError>;

    fn lookup_in_dir(&self, parent_inode: u64, path: &str) -> Result<u64, RofsError>;

    fn file_count(&self) -> usize;
}

#[derive(Clone, Copy, Debug)]
struct Node {
    content: NodeContent,
    name_index: u32,
}

#[derive(Clone, Copy, Debug)]
enum NodeContent {
    Dir {
        child_index: NonZero<u32>,
        child_count: u32,
    },
    File {
        data_index: u32,
    },
}

impl<T, C: Config> Rofs<T, C> {
    #[inline]
    fn node_entry(&self, node: &Node) -> Entry<'_, T> {
        let index = self
            .nodes
            .element_offset(node)
            .expect("must belong to this filesystem");

        let inode = (index as u64).wrapping_sub(C::INODE_ROOT);
        let name = self.node_name(node);

        let kind = match node.content {
            NodeContent::Dir {
                child_index,
                child_count,
            } => {
                let start = child_index.get() as usize;
                let end = start + child_count as usize;

                let iter = self.nodes[start..end]
                    .iter()
                    .map(|node| self.node_entry(node));

                EntryKind::Dir(Box::new(iter))
            }
            NodeContent::File { data_index } => {
                let data_index = data_index as usize;
                EntryKind::File(&self.files[data_index])
            }
        };

        Entry { inode, kind, name }
    }

    #[inline]
    fn node_name(&self, node: &Node) -> &str {
        assert!(
            self.nodes.as_ptr_range().contains(&&raw const *node),
            "must belong to this filesystem",
        );

        // SAFETY: This node belongs to this filesystem, so same as in `Rofs::new`.
        // Note this wouldn't be safe in the archived version.
        unsafe {
            let name = self.names.get_unchecked(node.name_index()..);
            let (len, rest) = name.split_first().unwrap_unchecked();
            let bytes = rest.get_unchecked(..*len as usize);

            str::from_utf8_unchecked(bytes)
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

            index = self.nodes[start..end]
                .eytzinger_search_by_key(&component, |node| self.node_name(node))?
                + start;

            if components.peek().is_none() {
                break Some(index as u32);
            }
        }
    }
}

impl Node {
    fn dir(child_index: NonZero<u32>, child_count: u32, name_index: u32) -> Self {
        Self {
            content: NodeContent::Dir {
                child_index,
                child_count,
            },
            name_index,
        }
    }

    fn file(data_index: u32, name_index: u32) -> Self {
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

impl<T, C: Config> ReadOnlyFilesystem for Rofs<T, C> {
    type File = T;

    #[inline]
    fn name(&self, inode: u64) -> Result<&str, RofsError> {
        let index = inode.wrapping_sub(C::INODE_ROOT) as usize;
        let node = self.nodes.get(index).ok_or(RofsError::NotFound)?;
        Ok(self.node_name(node))
    }

    #[inline]
    fn entry(&self, inode: u64) -> Result<Entry<'_, Self::File>, RofsError> {
        let index = inode.wrapping_sub(C::INODE_ROOT) as usize;
        let node = self.nodes.get(index).ok_or(RofsError::NotFound)?;
        Ok(self.node_entry(node))
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
    fn file_count(&self) -> usize {
        self.files.len()
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

impl<T: fmt::Debug, C: Config> fmt::Debug for Rofs<T, C> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Rofs")
            .field("nodes", &self.nodes)
            .field("files", &self.files)
            .field("paths", &self.names)
            .finish()
    }
}

impl<T: fmt::Debug> fmt::Debug for EntryKind<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Dir(_) => f.debug_tuple("EntryKind::Dir").finish_non_exhaustive(),
            Self::File(file) => f.debug_tuple("EntryKind::File").field(file).finish(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{fmt, fs, sync::LazyLock};

    use crate::{
        filesystem::{Config, EntryKind, ReadOnlyFilesystem, Rofs, object::RofsObject},
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
        assert_eq!(f.lookup("").unwrap(), 1);

        assert_eq!(f.name(1).unwrap(), "");

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
            let EntryKind::File(&hash) = f.entry(inode).unwrap().kind else {
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
        static FS: LazyLock<BndFs> = LazyLock::new(|| {
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

            let fs = files
                .iter()
                .map(|file| {
                    RofsObject::new(file.lines().map(|path| (path, hash_path32(path).unwrap())))
                })
                .fold(RofsObject::default(), RofsObject::merge)
                .into_rofs();

            fs::write(
                "out.txt",
                fs.nodes
                    .iter()
                    .map(|node| fs.node_name(node))
                    .collect::<Vec<_>>()
                    .join("\n"),
            )
            .unwrap();

            fs
        });

        &FS
    }
}
