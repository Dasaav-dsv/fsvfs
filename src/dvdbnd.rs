use std::{
    collections::VecDeque,
    env,
    ffi::OsStr,
    fs::{self, DirEntry},
    io, iter,
    path::{Path, PathBuf},
    sync::LazyLock,
};

use color_eyre::eyre;
use tracing::info;

use crate::{
    cache::Cache,
    cli::DvdbndArgs,
    dvdbnd::{dict::Dictionary, keys::KeyProvider, mount::DvdbndMount, path::ArchivePaths},
    time::time,
};

mod bhd5;
mod dict;
pub mod filesystem;
mod keys;
mod mount;
mod path;

pub fn mount(
    mountpoint: &str,
    archives: &[impl AsRef<Path>],
    game: Option<&str>,
    keys_dir: &Path,
    dict_dir: &Path,
    cache: Cache,
) -> eyre::Result<()> {
    let archives = ArchivePaths::new(archives);

    let keys = time!(
        KeyProvider::new(&archives, keys_dir).into_keys_for_game(game)?,
        |t| info!("matched BHD5 keys ({t:.02?})"),
    );

    let dict = time!(
        match keys.game.as_deref() {
            Some(game) => Some(Dictionary::from_dir_and_game(dict_dir, game)?),
            None => None,
        },
        |t| info!(
            "built dictionary for \"{}\" ({t:.02?})",
            keys.game.as_deref().unwrap_or("unknown game")
        ),
    );

    let mount = DvdbndMount::from_keys_and_dict(&keys, dict.as_ref(), cache)?;
    mount.mount(mountpoint)?;

    Ok(())
}

pub fn mount_from_args(args: DvdbndArgs) -> eyre::Result<()> {
    let app_dir = app_dir();
    tracing::debug!(?app_dir);

    let keys_dir = args
        .keys
        .as_deref()
        .map_or_else(|| app_dir.join("dvdbnd/Key"), PathBuf::from);

    let dict_dir = args
        .dict
        .as_deref()
        .map_or_else(|| app_dir.join("dvdbnd/Hash"), PathBuf::from);

    let cache_dir = args
        .cache
        .cache
        .as_deref()
        .map_or_else(|| app_dir.join("cache"), PathBuf::from);

    tracing::info!(?keys_dir, ?dict_dir, ?cache_dir);

    let cache = match args.cache.no_cache {
        Some(true) => Cache::empty(),
        _ => Cache::open(&cache_dir),
    };

    mount(
        &args.mountpoint,
        &args.archive,
        args.game.as_deref(),
        &keys_dir,
        &dict_dir,
        cache,
    )
}

fn app_dir() -> &'static Path {
    static PATH: LazyLock<PathBuf> = LazyLock::new(|| {
        if let Some(mut path) = env::args_os().next().map(PathBuf::from)
            && path.pop()
        {
            path
        } else {
            tracing::warn!("argv[0] is not set?");
            PathBuf::from(".")
        }
    });

    &PATH
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

// FIXME
fn prefix_and_parent_to_lowercase(path: &Path) -> (String, String) {
    let prefix = path
        .file_prefix()
        .and_then(OsStr::to_str)
        .unwrap_or_default();

    let parent = path
        .parent()
        .and_then(|parent| parent.file_name()?.to_str())
        .unwrap_or_default();

    (prefix.to_ascii_lowercase(), parent.to_ascii_lowercase())
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
