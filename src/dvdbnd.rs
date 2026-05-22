use std::{
    collections::VecDeque,
    ffi::OsStr,
    fs::{self, DirEntry},
    io, iter,
    path::{Path, PathBuf},
};

use color_eyre::eyre;
use tracing::instrument;

use crate::{
    cli::DvdbndArgs,
    dvdbnd::{dict::Dictionary, keys::KeyProvider, path::ArchivePaths},
};

mod bhd5;
mod dict;
mod filesystem;
mod keys;
mod path;

#[instrument(skip(archives), err)]
pub fn mount(
    root: &str,
    archives: &[impl AsRef<Path>],
    game: Option<&str>,
    keys_dir: &Path,
    dict_dir: &Path,
) -> eyre::Result<()> {
    let archives = ArchivePaths::new(archives);

    let keys = KeyProvider::new(&archives, keys_dir).into_keys_for_game(game)?;

    let dict = Dictionary::from_dir_and_game(dict_dir, game)?;

    Ok(())
}

pub fn mount_from_args(args: DvdbndArgs) -> eyre::Result<()> {
    mount(
        &args.root,
        &args.archive,
        args.game.as_deref(),
        args.keys.as_deref().unwrap_or("dvdbnd/Key").as_ref(),
        args.dict.as_deref().unwrap_or("dvdbnd/Hash").as_ref(),
    )
}

fn recursive_read_files(
    dir: &Path,
) -> io::Result<impl Iterator<Item = io::Result<PathBuf>> + 'static> {
    let mut dirs = VecDeque::new();
    let mut iter = fs::read_dir(dir)?;

    fn map_result(
        entry: io::Result<DirEntry>,
        dirs: &mut VecDeque<PathBuf>,
    ) -> Option<io::Result<PathBuf>> {
        let entry = match entry {
            Ok(entry) => entry,
            Err(e) => return Some(Err(e)),
        };

        let path = entry.path();

        match entry.file_type() {
            Ok(ty) if ty.is_dir() => {
                dirs.push_back(path);
                None
            }
            Ok(_) => Some(Ok(path)),
            Err(e) => Some(Err(e)),
        }
    }

    Ok(iter::from_fn(move || {
        loop {
            if let Some(entry_res) = iter.next() {
                if let Some(res) = map_result(entry_res, &mut dirs) {
                    return Some(res);
                }
                continue;
            }

            match fs::read_dir(dirs.pop_front()?) {
                Ok(next) => iter = next,
                Err(e) => return Some(Err(e)),
            }
        }
    }))
}

fn prefix_and_parent_to_lowercase(path: &Path) -> (Box<str>, Box<str>) {
    let prefix = path
        .file_prefix()
        .and_then(OsStr::to_str)
        .unwrap_or_default();
    let parent = path
        .parent()
        .and_then(|parent| parent.file_name()?.to_str())
        .unwrap_or_default();

    (
        prefix.to_ascii_lowercase().into_boxed_str(),
        parent.to_ascii_lowercase().into_boxed_str(),
    )
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use color_eyre::eyre;
    use fxhash::{FxBuildHasher, FxHashMap};

    use crate::{
        dvdbnd::{
            keys::{KeyProvider, Keys},
            path::ArchivePaths,
        },
        tests::SteamAppId,
    };

    impl SteamAppId {
        pub const fn game_name(self) -> &'static str {
            match self {
                Self::DarkSouls => "DarkSouls_PC",
                Self::DarkSouls2 => "DarkSouls2_PC",
                Self::DarkSouls2SotFS => "DarkSouls2Scholar_PC",
                Self::DarkSouls3 => "DarkSouls3_PC",
                Self::DarkSoulsRemastered => "DarkSouls_PC",
                Self::Sekiro => "Sekiro_PC",
                Self::SekiroSoundtrack => "SekiroSoundtrack_PC",
                Self::EldenRing => "EldenRing_PC",
                Self::ArmoredCore6 => "ArmoredCore6_PC",
                Self::Nightreign => "EldenRingNightreign_PC",
            }
        }

        pub const fn bhd_paths(self) -> &'static [&'static str] {
            match self {
                Self::DarkSouls => &[
                    "DATA/dvdbnd0.bhd5",
                    "DATA/dvdbnd1.bhd5",
                    "DATA/dvdbnd2.bhd5",
                    "DATA/dvdbnd3.bhd5",
                ],
                Self::DarkSouls2 => &[
                    "Game/GameDataEbl.bhd",
                    "Game/HqChrEbl.bhd",
                    "Game/HqMapEbl.bhd",
                    "Game/HqObjEbl.bhd",
                    "Game/HqPartsEbl.bhd",
                ],
                Self::DarkSouls2SotFS => &[
                    "Game/GameDataEbl.bhd",
                    "Game/LqChrEbl.bhd",
                    "Game/LqMapEbl.bhd",
                    "Game/LqObjEbl.bhd",
                    "Game/LqPartsEbl.bhd",
                ],
                Self::DarkSouls3 => &[
                    "Game/Data1.bhd",
                    "Game/Data2.bhd",
                    "Game/Data3.bhd",
                    "Game/Data4.bhd",
                    "Game/Data5.bhd",
                    "Game/DLC1.bhd",
                    "Game/DLC2.bhd",
                ],
                Self::DarkSoulsRemastered => &[],
                Self::Sekiro => &[
                    "Data1.bhd",
                    "Data2.bhd",
                    "Data3.bhd",
                    "Data4.bhd",
                    "Data5.bhd",
                ],
                Self::SekiroSoundtrack => &["Data.bhd"],
                Self::EldenRing => &[
                    "Game/sd/sd.bhd",
                    "Game/sd/sd_dlc02.bhd",
                    "Game/Data0.bhd",
                    "Game/Data1.bhd",
                    "Game/Data2.bhd",
                    "Game/Data3.bhd",
                    "Game/DLC.bhd",
                ],
                Self::ArmoredCore6 => &[
                    "Game/sd/sd.bhd",
                    "Game/Data0.bhd",
                    "Game/Data1.bhd",
                    "Game/Data2.bhd",
                    "Game/Data3.bhd",
                ],
                Self::Nightreign => &[
                    "Game/sd/sd.bhd",
                    "Game/sd/sd_dlc01.bhd",
                    "Game/data0.bhd",
                    "Game/data1.bhd",
                    "Game/data2.bhd",
                    "Game/data3.bhd",
                    "Game/dlc01.bhd",
                ],
            }
        }

        pub fn bhd_keys(self) -> eyre::Result<Keys<'static>> {
            static KEYS: Mutex<FxHashMap<SteamAppId, &'static ArchivePaths>> =
                Mutex::new(FxHashMap::with_hasher(FxBuildHasher::new()));

            let install_dir = self.install_dir().unwrap();

            let mut keys = match KEYS.lock() {
                Ok(keys) => keys,
                Err(poisoned) => {
                    KEYS.clear_poison();
                    poisoned.into_inner()
                }
            };

            let archives = keys.entry(self).or_insert_with(|| {
                let paths = self
                    .bhd_paths()
                    .into_iter()
                    .map(|path| install_dir.join(path));

                let archives = Box::new(ArchivePaths::new(paths));

                Box::leak(archives)
            });

            let game = Some(self.game_name());
            let keys = KeyProvider::new(archives, "dist/dvdbnd/Key".as_ref())
                .into_keys_for_game(game)
                .unwrap();

            Ok(keys)
        }
    }
}
