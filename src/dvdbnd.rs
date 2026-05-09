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
    dvdbnd::{dict::Dictionary, keys::KeysProvider, paths::ArchivePaths},
};

mod bhd5;
mod dict;
mod hash;
mod keys;
mod paths;

#[instrument(skip(archives), err)]
pub fn mount(
    root: &str,
    archives: &[impl AsRef<Path>],
    game: Option<&str>,
    keys_dir: &Path,
    dict_dir: &Path,
) -> eyre::Result<()> {
    let archives = ArchivePaths::new(archives);

    let keys = KeysProvider::new(&archives, keys_dir).into_keys_for_game(game)?;

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
