use clap::Parser;
use color_eyre::eyre;
use tracing::{debug, instrument};

use crate::cli::{Cli, Command};

mod cli;
mod crypto;
mod dvdbnd;
#[cfg(windows)]
mod winfsp;

#[instrument(err)]
fn main() -> eyre::Result<()> {
    color_eyre::install()?;

    let cli = Cli::try_parse()?;
    debug!("parsed CLI: {cli:?}");

    match cli.command {
        Command::Dvdbnd(args) => dvdbnd::mount_from_args(args),
    }
}

#[cfg(test)]
mod tests {
    use std::{env, path::Path, sync::Once};

    use color_eyre::eyre;

    #[track_caller]
    pub fn with_steam_game_dir<F: FnOnce(&Path)>(app_id: u32, f: F) {
        if env::var_os("NO_STEAM_TESTS").is_some_and(|var| var == "1") {
            return;
        }

        fn locate(app_id: u32) -> eyre::Result<std::path::PathBuf> {
            for steam_dir in steamlocate::locate_all()? {
                if let Ok(Some((app, lib))) = steam_dir.find_app(app_id) {
                    return Ok(lib.resolve_app_dir(&app));
                }
            }

            Err(eyre::eyre!("not found in any Steam library"))
        }

        match locate(app_id) {
            Ok(app_path) => f(&app_path),
            Err(e) => {
                let mut e = Some(e);

                static ONCE: Once = Once::new();
                ONCE.call_once(|| e = Some(eyre::eyre!(
                    "{}\n(to suppress tests that require certain Steam games run with `NO_STEAM_TESTS=1` environment variable)",
                    e.take().unwrap()
                )));

                panic!(
                    "unable to locate installation for Steam game with app id ({app_id}): {}",
                    e.unwrap()
                )
            }
        }
    }
}
