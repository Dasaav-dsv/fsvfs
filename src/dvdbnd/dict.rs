use std::path::Path;

use color_eyre::eyre;
use fxhash::FxHashMap;

use crate::dvdbnd::{prefix_and_parent_to_lowercase, recursive_read_files};

#[derive(Default, Debug)]
pub struct Dictionary {
    by_game: FxHashMap<Box<str>, Paths>,
}

#[derive(Default, Debug)]
struct Paths {
    by_bnd: FxHashMap<Box<str>, Box<Path>>,
}

impl Dictionary {
    pub fn from_dir_and_game(dir: &Path, game: Option<&str>) -> eyre::Result<Self> {
        let mut dict = Self::default();

        for file in recursive_read_files(dir)? {
            let file = file?;
            if file
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("txt"))
            {
                let prefix_and_parent = prefix_and_parent_to_lowercase(&file);
                if let Some(game) = game
                    && !game.eq_ignore_ascii_case(&prefix_and_parent.1)
                {
                    continue;
                }

                let (name, game) = prefix_and_parent;

                dict.by_game
                    .entry(game)
                    .or_insert_with(Paths::default)
                    .by_bnd
                    .insert(name, file.into_boxed_path());
            }
        }

        Ok(dict)
    }
}

#[cfg(test)]
mod tests {
    use crate::dvdbnd::dict::Dictionary;

    #[test]
    fn dictionary_from_dir() {
        let _dict = Dictionary::from_dir_and_game("dist/dvdbnd/Hash".as_ref(), None).unwrap();
    }

    #[test]
    fn dictionary_from_dir_and_game() {
        let dict = Dictionary::from_dir_and_game("dist/dvdbnd/Hash".as_ref(), Some("DarkSouls_PC"))
            .unwrap();

        let games = dict.by_game.keys().collect::<Vec<_>>();

        assert_eq!(games.len(), 1);
        assert_eq!(&**games[0], "darksouls_pc");
    }
}
