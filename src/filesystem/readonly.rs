use std::{borrow::Cow, marker::PhantomData, mem, num::NonZero};

use smallvec::SmallVec;

use crate::filesystem::paths::Paths;

#[derive(Debug)]
pub struct RofsBuilder<'a, C: Config> {
    files: Vec<(&'a str, C::File)>,
}

#[derive(Debug)]
pub struct Rofs<C: Config> {
    nodes: Vec<Node>,
    files: Vec<C::File>,
    paths: Paths,
}

pub trait Config {
    type File;

    const SEPARATORS: &[char] = &['/'];

    #[inline]
    fn normalize_path(path: &str) -> String {
        let mut components = SmallVec::<[Box<str>; 12]>::new_const();

        for component in path.split(Self::SEPARATORS) {
            match component {
                "" | "." => {}
                ".." => {
                    if components.last().is_some_and(|parent| &**parent != "..") {
                        components.pop();
                    }
                }
                _ => {
                    components.push(Self::normalize_component(component).into_boxed_str());
                }
            }
        }

        components.join("/")
    }

    #[inline]
    fn normalize_component(component: &str) -> String {
        component.to_string()
    }
}

#[derive(Clone, Copy, Debug)]
enum Node {
    Dir(DirNode),
    File(FileNode),
}

#[derive(Clone, Copy, Debug)]
struct DirNode {
    child_index: NonZero<u32>,
    child_count: NonZero<u32>,
}

#[derive(Clone, Copy, Debug)]
struct FileNode {
    data_index: u32,
}

impl<'a, C: Config> RofsBuilder<'a, C> {
    pub const fn new() -> Self {
        Self { files: Vec::new() }
    }

    pub fn with_files<I>(&mut self, iter: I) -> &mut Self
    where
        I: IntoIterator<Item = (&'a str, C::File)>,
    {
        self.files.extend(iter);
        self
    }

    pub fn finish(&mut self) -> Rofs<C> {
        Rofs::new(mem::take(&mut self.files))
    }
}

impl<C: Config> Rofs<C> {
    fn new(files: Vec<(&str, C::File)>) -> Self {
        assert!(
            files.len() <= u32::MAX as usize,
            "more files than available inodes",
        );

        let (mut paths, files) = files
            .into_iter()
            .enumerate()
            .map(|(i, (path, file))| {
                let normalized = C::normalize_path(path);
                let file_index = FileNode {
                    data_index: i as u32,
                };
                ((normalized, file_index), file)
            })
            .unzip::<_, _, Vec<_>, Vec<_>>();

        paths.sort_by(|a, b| a.0.cmp(&b.0));

        todo!()
    }

    fn file_data(&self, node: FileNode) -> &C::File {
        &self.files[node.data_index as usize]
    }
}

impl<C: Config> Default for RofsBuilder<'_, C> {
    fn default() -> Self {
        Self::new()
    }
}
