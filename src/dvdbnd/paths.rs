use std::{collections::BTreeSet, ops::Bound, path::Path, rc::Rc};

use fxhash::FxBuildHasher;
use indexmap::IndexMap;

#[derive(Default, Debug)]
pub struct ArchivePaths {
    pub paths: IndexMap<Rc<str>, Box<Path>, FxBuildHasher>,
    pub names: BTreeSet<Rc<str>>,
}

impl ArchivePaths {
    pub fn new(archive_paths: impl IntoIterator<Item: AsRef<Path>>) -> Self {
        let (paths, names) = archive_paths
            .into_iter()
            .filter_map(|path| {
                let path = path.as_ref();

                let mut name = Rc::<str>::from(path.file_name()?.to_str()?);
                Rc::make_mut(&mut name).make_ascii_lowercase();

                let path = (name.clone(), Box::from(path));

                Some((path, name))
            })
            .unzip();

        Self { paths, names }
    }

    pub fn names_by_prefix<'a>(
        &'a self,
        prefix: &str,
    ) -> impl Iterator<Item = &'a Rc<str>> + use<'a> {
        let start = prefix.to_ascii_lowercase();
        let end = start.clone() + "/";
        self.names.range::<str, _>((
            Bound::Included(start.as_str()),
            Bound::Excluded(end.as_str()),
        ))
    }
}

#[cfg(test)]
mod tests {
    use crate::dvdbnd::paths::ArchivePaths;

    const ARCHIVES: [&str; 4] = [
        "Game/Data2.bdt",
        "Game/Data2.bhd",
        "Game/Data1.bdt",
        "Game/Data1.bhd",
    ];

    #[test]
    fn archive_paths_order() {
        let archives = ArchivePaths::new(ARCHIVES);

        assert!(
            archives
                .paths
                .values()
                .filter_map(|path| path.to_str())
                .eq(ARCHIVES)
        );
    }

    #[test]
    fn archive_paths_names() {
        let archives = ArchivePaths::new(ARCHIVES);

        assert!(archives.names.iter().map(|name| &**name).eq([
            "data1.bdt",
            "data1.bhd",
            "data2.bdt",
            "data2.bhd",
        ]));
    }

    #[test]
    fn archive_paths_names_by_prefix() {
        let archives = ArchivePaths::new(ARCHIVES);

        assert!(
            archives
                .names_by_prefix("data1")
                .map(|name| &**name)
                .eq(["data1.bdt", "data1.bhd"])
        );
    }
}
