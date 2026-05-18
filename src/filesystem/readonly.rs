use std::{collections::VecDeque, fmt, marker::PhantomData, mem, num::NonZero};

use fxhash::FxHashMap;

use crate::filesystem::paths::Paths;

#[derive(Debug)]
pub struct RofsBuilder<'a, T> {
    files: Vec<(&'a str, T)>,
}

#[derive(Debug)]
pub struct Rofs<T, C: Config = DefaultConfig> {
    nodes: Box<[Node]>,
    files: Box<[T]>,
    paths: Paths,
    _config: PhantomData<C>,
}

#[derive(Clone, Copy, Debug)]
pub struct DefaultConfig;

impl Config for DefaultConfig {}

pub trait Config {
    const SEPARATORS: &[char] = &['/'];

    #[inline]
    fn normalized_components(path: &str) -> Vec<String> {
        let mut components = Vec::<String>::new();

        for component in path.split(Self::SEPARATORS) {
            match component {
                "" | "." => {}
                ".." => {
                    components.pop_if(|parent| parent != "..");
                }
                _ => {
                    components.push(Self::normalize_component(component));
                }
            }
        }

        components
    }

    #[inline]
    fn normalize_component(component: &str) -> String {
        component.to_string()
    }
}

#[derive(Clone, Copy, Debug)]
enum Node {
    Dir {
        child_index: NonZero<u32>,
        child_count: NonZero<u32>,
    },
    File(FileNode),
}

#[derive(Clone, Copy, Debug)]
struct FileNode {
    data_index: u32,
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
        Rofs::new(mem::take(&mut self.files))
    }
}

impl<T, C: Config> Rofs<T, C> {
    fn new(files: Vec<(&str, T)>) -> Self {
        let _ = Self::inode_from(files.len());

        let (components, files) = files
            .into_iter()
            .enumerate()
            .map(|(i, (path, file))| {
                let components = C::normalized_components(path);
                let file_index = FileNode {
                    data_index: i as u32,
                };
                ((components, file_index), file)
            })
            .unzip::<_, _, Vec<_>, Vec<_>>();

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
        let mut paths = Vec::<(u32, String)>::with_capacity(total as usize);

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
                            None => "/".to_string(),
                            Some("/") => component.to_string(),
                            Some(parent) => [parent, *component].join("/"),
                        };
                        let parent_index = paths.len();

                        queue.push_back((parent_index, branch));

                        paths.push((inode, path));
                        inode += 1;
                    }
                    TreeNode::Leaf(file_node) => {
                        nodes.push(Node::File(*file_node));

                        let path = match paths[parent_index].1.as_str() {
                            "/" => component.to_string(),
                            parent => [parent, *component].join("/"),
                        };

                        paths.push((inode, path));
                        inode += 1;
                    }
                }
            }
        }

        let paths_iter = paths.iter().map(|(i, p)| (*i, p.as_str()));

        Self {
            nodes: nodes.into_boxed_slice(),
            files: files.into_boxed_slice(),
            paths: Paths::from_iter(paths_iter),
            _config: PhantomData,
        }
    }

    fn file_data(&self, node: FileNode) -> &T {
        &self.files[node.data_index as usize]
    }

    #[track_caller]
    fn inode_from<N>(n: N) -> u32
    where
        N: TryInto<u32> + fmt::Display + Copy,
    {
        if let Ok(n) = n.try_into() {
            return n;
        }

        panic!("inode conversion failed: input ({n}) is larger than `u32::MAX`!");
    }
}

impl<C: Config> Default for RofsBuilder<'_, C> {
    fn default() -> Self {
        Self::new()
    }
}
