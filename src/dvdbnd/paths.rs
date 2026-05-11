use std::{
    ffi::OsStr,
    ops::Deref,
    path::{Path, PathBuf},
};

use fxhash::FxBuildHasher;
use indexmap::IndexMap;

use crate::dvdbnd::bhd5::strip_bhd_extension;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BhdPath(Box<Path>);

#[derive(Default, Debug)]
pub struct ArchivePaths {
    pub paths: IndexMap<Box<str>, BhdPath, FxBuildHasher>,
}

impl BhdPath {
    pub fn new<P: Into<Box<Path>>>(path: P) -> Self {
        let path = path.into();
        debug_assert_eq!(path.extension(), Some(OsStr::new("bhd")));
        Self(path)
    }

    pub fn as_bhd(&self) -> &Path {
        &self.0
    }

    pub fn to_bdt(&self) -> PathBuf {
        self.0.with_extension("bdt")
    }
}

impl ArchivePaths {
    pub fn new(archive_paths: impl IntoIterator<Item: AsRef<Path>>) -> Self {
        let paths = archive_paths
            .into_iter()
            .filter_map(|path| {
                let path = path.as_ref();
                let name = path.file_name()?.to_str().and_then(strip_bhd_extension)?;
                Some((name.to_ascii_lowercase().into(), BhdPath::new(path)))
            })
            .collect();

        Self { paths }
    }
}

impl AsRef<Path> for BhdPath {
    #[inline]
    fn as_ref(&self) -> &Path {
        self.as_bhd()
    }
}

impl Deref for BhdPath {
    type Target = Path;

    #[inline]
    fn deref(&self) -> &Self::Target {
        self.as_bhd()
    }
}

#[cfg(test)]
mod tests {
    use crate::dvdbnd::paths::ArchivePaths;

    #[test]
    fn archive_paths_order() {
        let archives = ArchivePaths::new([
            "Game/Data2.bdt",
            "Game/Data2.bhd",
            "Game/Data1.bdt",
            "Game/Data1.bhd",
        ]);

        assert!(
            archives
                .paths
                .iter()
                .filter_map(|(name, path)| Some((path.to_str()?, &**name)))
                .eq([("Game/Data2.bhd", "data2"), ("Game/Data1.bhd", "data1")])
        );
    }
}
