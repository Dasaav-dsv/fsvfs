use std::{
    fs, io,
    path::{Path, PathBuf},
    rc::Rc,
};

use color_eyre::eyre::{self, OptionExt};
use fxhash::FxHashMap;

use crate::{
    crypto::rsa::RsaKey,
    dvdbnd::{
        bhd5::has_bhd_extension, paths::ArchivePaths, prefix_and_parent_to_lowercase,
        recursive_read_files,
    },
};

pub struct KeysProvider<'a, 'k> {
    archives: &'a ArchivePaths,
    keys_dir: &'k Path,
    by_game: FxHashMap<Box<str>, FxHashMap<Box<str>, RsaKey>>,
}

#[derive(Default, Debug)]
pub struct Keys {
    game: Box<str>,
    by_bnd: FxHashMap<Box<str>, RsaKey>,
}

impl<'a, 'k> KeysProvider<'a, 'k> {
    pub fn new(archives: &'a ArchivePaths, keys_dir: &'k Path) -> Self {
        Self {
            archives,
            keys_dir,
            by_game: Default::default(),
        }
    }

    pub fn into_keys_for_game(self, game: Option<&str>) -> eyre::Result<Keys> {
        self.into_keys_for_game_with_filter(game, |bnd_path, key| true)
    }

    fn into_keys_for_game_with_filter<F>(
        mut self,
        mut game: Option<&str>,
        mut f: F,
    ) -> eyre::Result<Keys>
    where
        F: FnMut(&Path, &RsaKey) -> bool,
    {
        for pem_file in self.pem_files_iter()? {
            let pem_file = pem_file?;

            let (prefix, parent) = prefix_and_parent_to_lowercase(&pem_file);

            if let Some(game) = game
                && !parent.eq_ignore_ascii_case(game)
            {
                continue;
            }

            let mut bhd_names = self.bhd_names_by_prefix(&prefix).peekable();
            if bhd_names.peek().is_none() {
                continue;
            }

            let pem = fs::read_to_string(&pem_file)?;
            let key = RsaKey::decode_from_pem(&pem)?;

            self.by_game.entry(parent).or_default().insert(prefix, key);
        }

        let game = game.ok_or_eyre("unable to determine key for archives")?;
        let by_bnd = self.by_game.remove(game).unwrap_or_default();

        Ok(Keys {
            game: game.into(),
            by_bnd,
        })
    }

    fn pem_files_iter(&self) -> eyre::Result<impl Iterator<Item = io::Result<PathBuf>> + 'static> {
        Ok(
            recursive_read_files(self.keys_dir)?.filter_map(|file| match file {
                Ok(file) => file
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("pem"))
                    .then_some(Ok(file)),
                res => Some(res),
            }),
        )
    }

    fn bhd_names_by_prefix(&self, prefix: &str) -> impl Iterator<Item = &'a Rc<str>> + use<'a> {
        self.archives
            .names_by_prefix(prefix)
            .filter(|name| has_bhd_extension(&***name))
    }
}
