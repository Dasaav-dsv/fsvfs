use std::{
    fs::{self, File},
    io::{self, Read},
    path::Path,
    slice,
};

use color_eyre::eyre;
use fxhash::FxHashMap;
use smallvec::{SmallVec, smallvec_inline};
use tracing::warn;
use zerocopy::{BE, LE, TryFromBytes};

use crate::{
    crypto::rsa::{RsaDecryptor, RsaKey},
    dvdbnd::{
        bhd5::format::{BHD5_HEADER_LEN, Header as Bhd5Header},
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
    pub game: Option<Box<str>>,
    pub by_path: FxHashMap<&'a BhdPath, Option<RsaKey>>,
}

type PemPathMap = Vec<(Box<str>, SmallVec<[(Box<str>, Box<Path>); 1]>)>;

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

        let mut game = game;
        let mut game_index = None;

        let mut cache = FxHashMap::default();
        let mut by_path = FxHashMap::default();

        for (name, bhd_path) in &self.archives.paths {
            if f(bhd_path, None)? {
                by_path.insert(bhd_path, None);
                continue;
            }

            for (game_name, pem_paths) in game_index
                .or_else(|| {
                    game_index = game
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
                        game.get_or_insert(game_name);
                    }
                }
            }
        }

        Ok(Keys {
            game: game.map(Box::from),
            by_path,
        })
    }

    fn find_pem_paths(&self) -> io::Result<PemPathMap> {
        recursive_read_files(self.keys_dir)?
            .filter_map(|file| match file {
                Ok(file) => file
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("pem"))
                    .then_some(Ok(file)),
                res => Some(res),
            })
            .try_fold(PemPathMap::new(), |mut map, path| -> io::Result<_> {
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
        tests::SteamAppId,
    };

    #[test]
    #[ignore]
    fn ds2_detect_keys() {
        keys_for_detected_game(SteamAppId::DarkSouls2);
    }

    #[test]
    #[ignore]
    fn ds2s_detect_keys() {
        keys_for_detected_game(SteamAppId::DarkSouls2SotFS);
    }

    #[test]
    #[ignore]
    fn ds3_detect_keys() {
        keys_for_detected_game(SteamAppId::DarkSouls2);
    }

    #[test]
    #[ignore]
    fn sekiro_detect_keys() {
        keys_for_detected_game(SteamAppId::Sekiro);
    }

    #[test]
    #[ignore]
    fn sekiro_ost_detect_keys() {
        keys_for_detected_game(SteamAppId::SekiroSoundtrack);
    }

    #[test]
    #[ignore]
    fn er_detect_keys() {
        keys_for_detected_game(SteamAppId::EldenRing);
    }

    #[test]
    #[ignore]
    fn ac6_detect_keys() {
        keys_for_detected_game(SteamAppId::ArmoredCore6);
    }

    #[test]
    #[ignore]
    fn nr_detect_keys() {
        keys_for_detected_game(SteamAppId::Nightreign);
    }

    #[track_caller]
    fn keys_for_detected_game(game: SteamAppId) {
        let install_dir = game.install_dir().unwrap();
        let bhds = game.bhd_paths();

        let archives = ArchivePaths::new(bhds.into_iter().map(|bhd| install_dir.join(bhd)));
        let keys = KeyProvider::new(&archives, "dist/dvdbnd/Key".as_ref())
            .into_keys_for_game(None)
            .unwrap();

        assert_eq!(
            keys.game.as_deref(),
            Some(&*game.game_name().to_ascii_lowercase())
        );

        assert_eq!(keys.by_path.len(), bhds.len());
    }
}
