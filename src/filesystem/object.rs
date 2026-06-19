use std::{
    borrow::Cow,
    collections::{HashMap, VecDeque, hash_map::Entry},
    fmt,
    hash::BuildHasherDefault,
    marker::PhantomData,
    mem,
    num::NonZero,
};

use eytzinger::{SliceExt, permutation::InplacePermutator};
use rkyv::{Archive, Deserialize, Serialize, hash::FxHasher64};
type FxHashMap<K, V> = HashMap<K, V, BuildHasherDefault<FxHasher64>>;

use crate::{
    cow::CowExt,
    filesystem::{Config, DefaultConfig, Node, Rofs, components, normalize_path},
};

#[derive(Archive, Serialize, Deserialize)]
pub struct RofsObject<T, C: Config = DefaultConfig> {
    root: Branch,
    files: Vec<T>,
    _config: PhantomData<C>,
}

trait BranchExt: Sized {
    fn adjust_files(&mut self, offset: u32);

    fn merge(&mut self, other_branch: Self);
}

type Branch = FxHashMap<Box<str>, TreeNode>;

#[derive(Archive, Serialize, Deserialize)]
#[rkyv(bytecheck(bounds(__C: rkyv::validation::ArchiveContext)))]
#[rkyv(serialize_bounds(
    __S:rkyv::ser::Writer + rkyv::ser::Allocator,
    __S::Error: rkyv::rancor::Source
))]
#[rkyv(deserialize_bounds(__D::Error: rkyv::rancor::Source))]
enum TreeNode {
    Branch(#[rkyv(omit_bounds)] Branch),
    Leaf(u32),
}

impl<T, C: Config> RofsObject<T, C> {
    pub fn new<'a, S>(files: impl IntoIterator<Item = (&'a S, T)>) -> Self
    where
        S: AsRef<str> + ?Sized + 'a,
    {
        let (file_paths, files): (Vec<_>, Vec<_>) = files
            .into_iter()
            .zip(0..)
            .map(|((path, file), data_index)| {
                let mut path = normalize_path::<C>(path.as_ref());
                Cow::to_ascii_lowercase(&mut path);
                ((path, data_index), file)
            })
            .unzip();

        assert_u32(files.len());

        let mut root = FxHashMap::default();

        for (file_path, file_node) in &file_paths {
            let mut node = &mut root;

            let mut components = components::<C>(file_path);

            while let Some(component) = components.next() {
                assert!(
                    component.len() <= u8::MAX as usize,
                    "file names must be shorter than 256 bytes ({component})",
                );

                let next = match node.get_mut(component) {
                    None => match node.entry(Box::from(component)) {
                        Entry::Vacant(entry) => entry.insert(match components.peek() {
                            Some(_) => TreeNode::Branch(Default::default()),
                            None => TreeNode::Leaf(*file_node),
                        }),
                        Entry::Occupied(_) => unreachable!(),
                    },
                    #[allow(clippy::deref_addrof)]
                    Some(next) => unsafe {
                        // Polonius borrowck workaround.
                        // SAFETY: this borrow is always exclusive to the one above.
                        &mut *&raw mut *next
                    },
                };

                match next {
                    TreeNode::Branch(next) => node = next,
                    TreeNode::Leaf(_) => break,
                }
            }
        }

        let root = FxHashMap::from_iter([(Box::from(""), TreeNode::Branch(root))]);

        Self {
            root,
            files,
            _config: PhantomData,
        }
    }

    pub fn merge(mut self, mut other: Self) -> Self {
        let self_len = self.root.len();
        let other_len = other.root.len();

        if other_len == 0 || self_len == 0 {
            return if self_len != 0 { self } else { other };
        }

        let files_offset = self.files.len() as u32;
        other.root.adjust_files(files_offset);

        self.files.extend(mem::take(&mut other.files));
        assert_u32(self.files.len());

        self.root.merge(other.root);

        self
    }

    pub fn into_rofs(self) -> Rofs<T, C> {
        let mut nodes = vec![];

        let mut names = vec![];
        let mut names_interned = FxHashMap::default();

        let mut queue = VecDeque::from([&self.root]);
        let mut child_index = NonZero::<u32>::MIN;

        while let Some(branch) = queue.pop_front() {
            let first = nodes.len();

            for (component, node) in branch {
                let name_index = *names_interned.entry(&**component).or_insert_with(|| {
                    let index = assert_u32(names.len());
                    names.push(component.len() as u8);
                    names.extend_from_slice(component.as_bytes());
                    index
                });

                match node {
                    TreeNode::Branch(branch) => {
                        let child_count = branch.len() as u32;

                        nodes.push(Node::dir(child_index, child_count, name_index));
                        child_index = child_index.checked_add(child_count).unwrap();

                        queue.push_back(branch);
                    }
                    TreeNode::Leaf(data_index) => {
                        nodes.push(Node::file(*data_index, name_index));
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
            files: self.files.into_boxed_slice(),
            _config: PhantomData,
        }
    }
}

#[inline]
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

impl BranchExt for Branch {
    fn adjust_files(&mut self, offset: u32) {
        for node in self.values_mut() {
            match node {
                TreeNode::Branch(branch) => branch.adjust_files(offset),
                TreeNode::Leaf(file_index) => *file_index += offset,
            }
        }
    }

    fn merge(&mut self, other_branch: Self) {
        for (name, other_node) in other_branch {
            match self.entry(name) {
                Entry::Occupied(mut entry) => match (entry.get_mut(), other_node) {
                    (TreeNode::Branch(branch), TreeNode::Branch(other_branch)) => {
                        branch.merge(other_branch);
                    }
                    (TreeNode::Leaf(_), TreeNode::Leaf(_)) => {
                        tracing::warn!("duplicate file: {}", entry.key())
                    }
                    _ => {
                        tracing::warn!("file shares its name with a directory: {}", entry.key());
                    }
                },
                Entry::Vacant(entry) => {
                    entry.insert(other_node);
                }
            }
        }
    }
}

impl<T, C: Config> Default for RofsObject<T, C> {
    fn default() -> Self {
        Self {
            root: FxHashMap::default(),
            files: vec![],
            _config: PhantomData,
        }
    }
}
