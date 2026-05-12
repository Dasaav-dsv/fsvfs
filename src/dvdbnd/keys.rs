use std::{
    fs::{self, File},
    io::{self, Read},
    path::Path,
    slice,
};

use color_eyre::eyre::{self, OptionExt};
use fxhash::FxHashMap;
use smallvec::{SmallVec, smallvec_inline};
use tracing::warn;
use zerocopy::{BE, LE, TryFromBytes};

use crate::{
    crypto::rsa::{RsaDecryptor, RsaKey},
    dvdbnd::{
        bhd5::{BHD5_HEADER_LEN, Bhd5Header},
        path::{ArchivePaths, BhdPath},
        prefix_and_parent_to_lowercase, recursive_read_files,
    },
};

pub struct KeyProvider<'a, 'k> {
    archives: &'a ArchivePaths,
    keys_dir: &'k Path,
}

#[derive(Default, Debug)]
pub struct Keys<'a> {
    game: Box<str>,
    by_path: FxHashMap<&'a BhdPath, Option<RsaKey>>,
}

type PemPathsMap = Vec<(Box<str>, SmallVec<[(Box<str>, Box<Path>); 1]>)>;

impl<'a, 'k> KeyProvider<'a, 'k> {
    pub fn new(archives: &'a ArchivePaths, keys_dir: &'k Path) -> Self {
        Self { archives, keys_dir }
    }

    pub fn into_keys_for_game(self, game: Option<&str>) -> eyre::Result<Keys<'a>> {
        self.into_keys_for_game_with_filter(game, move |bnd_path, key| {
            let mut header_bytes = [0; BHD5_HEADER_LEN];
            let mut reader = File::open(bnd_path)?;

            let res = if let Some(key) = key {
                let mut decryptor = RsaDecryptor::new(key, &mut reader);
                decryptor.read_exact(&mut header_bytes)
            } else {
                reader.read_exact(&mut header_bytes)
            };

            if let Err(e) = res {
                warn!("skipping {bnd_path:?}: {e}");
                return Ok(false);
            }

            let is_ok = Bhd5Header::<LE>::try_ref_from_bytes(&header_bytes).is_ok()
                || Bhd5Header::<BE>::try_ref_from_bytes(&header_bytes).is_ok();

            Ok(is_ok)
        })
    }

    fn into_keys_for_game_with_filter<F>(
        self,
        game: Option<&str>,
        mut f: F,
    ) -> eyre::Result<Keys<'a>>
    where
        F: FnMut(&Path, Option<&RsaKey>) -> eyre::Result<bool>,
    {
        let pem_paths = self.find_pem_paths()?;

        let mut game_name = game;
        let mut game_index = None;

        let mut cache = FxHashMap::default();
        let mut by_path = FxHashMap::default();

        for (name, bhd_path) in &self.archives.paths {
            if f(bhd_path, None)? {
                by_path.insert(bhd_path, None);
                continue;
            }

            for (game, pem_paths) in game_index
                .or_else(|| {
                    game_index = game_name
                        .and_then(|game| pem_paths.binary_search_by_key(&game, |(k, _)| k).ok());
                    game_index
                })
                .map(|i| slice::from_ref(&pem_paths[i]))
                .unwrap_or_else(|| pem_paths.as_slice())
            {
                for (_, pem_path) in pem_paths.iter().filter(|(pem_name, _)| pem_name == name) {
                    let key = match cache.get(&**pem_path) {
                        Some(key) => key,
                        None => {
                            let pem = fs::read_to_string(pem_path)?;
                            let key = RsaKey::decode_from_pem(&pem)?;
                            cache.entry(&**pem_path).or_insert(key)
                        }
                    };

                    if f(bhd_path, Some(&key))? {
                        by_path.insert(bhd_path, Some(key.clone()));
                        game_name.get_or_insert(game);
                    }
                }
            }
        }

        let game = game_name.ok_or_eyre("unable to determine key for archives")?;

        Ok(Keys {
            game: game.into(),
            by_path,
        })
    }

    fn find_pem_paths(&self) -> io::Result<PemPathsMap> {
        recursive_read_files(self.keys_dir)?
            .filter_map(|file| match file {
                Ok(file) => file
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("pem"))
                    .then_some(Ok(file)),
                res => Some(res),
            })
            .try_fold(PemPathsMap::new(), |mut map, path| -> io::Result<_> {
                let path = path?.into_boxed_path();
                let (name, game) = prefix_and_parent_to_lowercase(&path);
                match map.binary_search_by_key(&&*game, |(k, _)| k) {
                    Ok(i) => map[i].1.push((name, path)),
                    Err(i) => map.insert(i, (game, smallvec_inline![(name, path)])),
                }
                Ok(map)
            })
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        dvdbnd::{keys::KeyProvider, path::ArchivePaths},
        tests::with_steam_game_dir,
    };

    #[test]
    fn steam_game_keys_for_ds3() {
        with_steam_game_dir(374320, |ds3_dir| {
            const BHDS: [&str; 7] = [
                "Game/Data1.bhd",
                "Game/Data2.bhd",
                "Game/Data3.bhd",
                "Game/Data4.bhd",
                "Game/Data5.bhd",
                "Game/DLC1.bhd",
                "Game/DLC2.bhd",
            ];

            let archives = ArchivePaths::new(BHDS.into_iter().map(|bhd| ds3_dir.join(bhd)));
            let keys = KeyProvider::new(&archives, "dist/dvdbnd/Key".as_ref())
                .into_keys_for_game(None)
                .unwrap();

            assert_eq!(&*keys.game, "darksouls3_pc");
            assert_eq!(keys.by_path.len(), 7);
        });
    }

    #[test]
    fn steam_game_keys_for_er() {
        with_steam_game_dir(1245620, |er_dir| {
            const BHDS: [&str; 6] = [
                "Game/sd/sd.bhd",
                "Game/sd/sd_dlc02.bhd",
                "Game/Data1.bhd",
                "Game/Data2.bhd",
                "Game/Data3.bhd",
                "Game/DLC.bhd",
            ];

            let archives = ArchivePaths::new(BHDS.into_iter().map(|bhd| er_dir.join(bhd)));
            let keys = KeyProvider::new(&archives, "dist/dvdbnd/Key".as_ref())
                .into_keys_for_game(None)
                .unwrap();

            assert_eq!(&*keys.game, "eldenring_pc");
            assert_eq!(keys.by_path.len(), 6);
        });
    }
}
