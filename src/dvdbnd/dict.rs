use std::{borrow::Cow, fs, path::Path};

use color_eyre::eyre;
use fxhash::FxHashMap;

use crate::{
    cow::CowExt,
    dvdbnd::{prefix_and_parent_to_lowercase, recursive_read_files},
    hash::{hash_path32, hash_path64},
};

#[derive(Default, Debug)]
pub struct Dictionary {
    by_bnd: FxHashMap<Box<str>, String>,
}

impl Dictionary {
    pub fn from_dir_and_game(dir: &Path, game: &str) -> eyre::Result<Self> {
        let mut by_bnd = FxHashMap::default();

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

    #[inline]
    pub fn hash_path32_iter<'a>(&'a self, bnd_name: &str) -> impl Iterator<Item = (&'a str, u32)> {
        self.hash_path_iter(bnd_name, hash_path32)
    }

    #[inline]
    pub fn hash_path64_iter<'a>(&'a self, bnd_name: &str) -> impl Iterator<Item = (&'a str, u64)> {
        self.hash_path_iter(bnd_name, hash_path64)
    }

    #[inline]
    fn hash_path_iter<'a, F, T>(
        &'a self,
        bnd_name: &str,
        f: F,
    ) -> impl Iterator<Item = (&'a str, T)>
    where
        F: Fn(&str) -> Option<T> + 'static,
    {
        let mut bnd_name = Cow::Borrowed(bnd_name);
        Cow::make_ascii_lowercase(&mut bnd_name);

        self.by_bnd
            .get(&*bnd_name)
            .into_iter()
            .flat_map(|contents| contents.lines())
            .filter_map(move |path| {
                let hash = f(path)?;
                Some((path, hash))
            })
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

        let paths_and_hashes = dict.hash_path32_iter("dvdbnd3").take(4).collect::<Vec<_>>();

        assert_eq!(
            paths_and_hashes,
            [
                ("/msg/english/item.msgbnd.dcx", 1353983167),
                ("/msg/english/menu.msgbnd.dcx", 1995071881),
                ("/msg/french/item.msgbnd.dcx", 3004801203),
                ("/msg/french/menu.msgbnd.dcx", 3645889917)
            ]
        )
    }
}
