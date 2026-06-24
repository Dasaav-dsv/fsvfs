use std::{fs, path::Path};

use color_eyre::eyre;

use crate::{
    XxHashMap,
    dvdbnd::{prefix_and_parent_to_lowercase, recursive_read_files},
    hash::{hash_path32, hash_path64},
};

#[derive(Default, Debug)]
pub struct Dictionary {
    by_bnd: XxHashMap<Box<str>, String>,
}

impl Dictionary {
    pub fn from_dir_and_game(dir: &Path, game: &str) -> eyre::Result<Self> {
        let mut by_bnd = XxHashMap::default();

        for file in recursive_read_files(dir)? {
            let file = file?;
            if file
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("txt"))
            {
                let (prefix, parent) = prefix_and_parent_to_lowercase(&file);
                if !game.eq_ignore_ascii_case(&parent) {
                    continue;
                }

                let contents = fs::read_to_string(&file)?;

                by_bnd.insert(prefix.into(), contents);
            }
        }

        Ok(Self { by_bnd })
    }

    pub fn hash_paths32<'a>(&'a self, bnd_name: &str) -> XxHashMap<u32, &'a str> {
        self.hash_path_iter(bnd_name, hash_path32).collect()
    }

    pub fn hash_paths64<'a>(&'a self, bnd_name: &str) -> XxHashMap<u64, &'a str> {
        self.hash_path_iter(bnd_name, hash_path64).collect()
    }

    fn hash_path_iter<'a, F, H>(
        &'a self,
        bnd_name: &str,
        f: F,
    ) -> impl Iterator<Item = (H, &'a str)>
    where
        F: Fn(&str) -> Option<H> + 'static,
    {
        let bnd_name = bnd_name.to_ascii_lowercase();

        self.by_bnd
            .get(&*bnd_name)
            .into_iter()
            .flat_map(|contents| contents.lines())
            .filter_map(move |path| f(path).zip(Some(path)))
    }
}

#[cfg(test)]
mod tests {
    use crate::dvdbnd::dict::Dictionary;

    #[test]
    fn dictionary_from_dir_and_game() {
        let dict =
            Dictionary::from_dir_and_game("dist/dvdbnd/Hash".as_ref(), "DarkSouls_PC").unwrap();

        assert_eq!(dict.by_bnd.len(), 4);

        let paths = dict.hash_paths32("dvdbnd3");

        for (path, hash) in [
            ("/msg/english/item.msgbnd.dcx", 1353983167),
            ("/msg/english/menu.msgbnd.dcx", 1995071881),
            ("/msg/french/item.msgbnd.dcx", 3004801203),
            ("/msg/french/menu.msgbnd.dcx", 3645889917),
        ] {
            assert_eq!(paths.get(&hash), Some(&path), "{path}");
        }
    }
}
