use std::collections::HashMap;

use clap::Parser;
use tracing::debug;
use xxhash_rust::xxh3::Xxh3DefaultBuilder;

use crate::cli::{Cli, Command};

mod cache;
mod cli;
mod cow;
mod crypto;
mod dvdbnd;
mod filesystem;
mod hash;
mod thread;
mod time;
mod unaligned;

#[cfg(windows)]
mod runas;

type XxHashMap<K, V> = HashMap<K, V, Xxh3DefaultBuilder>;

fn main() -> eyre::Result<()> {
    simple_eyre::install()?;

    tracing_subscriber::fmt()
        .with_ansi(true)
        .without_time()
        .init();

    let cli = Cli::parse();
    debug!("parsed CLI: {cli:?}");

    match cli.command {
        Command::Dvdbnd(args) => dvdbnd::mount_from_args(args),
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    #[allow(unused)]
    #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
    #[repr(u32)]
    pub enum SteamAppId {
        DarkSouls = 211420,
        DarkSouls2 = 236430,
        DarkSouls2SotFS = 335300,
        DarkSouls3 = 374320,
        DarkSoulsRemastered = 570940,
        Sekiro = 814380,
        SekiroSoundtrack = 1039230,
        EldenRing = 1245620,
        ArmoredCore6 = 1888160,
        Nightreign = 2622380,
    }

    impl SteamAppId {
        pub fn install_dir(self) -> eyre::Result<PathBuf> {
            if self == Self::SekiroSoundtrack {
                let sekiro_dir = Self::Sekiro.install_dir()?;
                return Ok(sekiro_dir.join("Artwork_MiniSoundtrack"));
            }

            for steam_dir in steamlocate::locate_all()? {
                if let Ok(Some((app, lib))) = steam_dir.find_app(self as u32) {
                    return Ok(lib.resolve_app_dir(&app));
                }
            }

            Err(eyre::eyre!(
                "unable to locate installation for Steam game ({self:?}) with app id ({})",
                self as u32,
            ))
        }
    }
}
