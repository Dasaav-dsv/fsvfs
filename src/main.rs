use clap::Parser;
use color_eyre::eyre;
use tracing::{debug, instrument};

use crate::cli::{Cli, Command};

mod cli;
mod cow;
mod crypto;
mod dvdbnd;
mod filesystem;
mod hash;
mod unaligned;

#[instrument(err)]
fn main() -> eyre::Result<()> {
    color_eyre::install()?;
    tracing_subscriber::fmt().init();

    let cli = Cli::try_parse()?;
    debug!("parsed CLI: {cli:?}");

    match cli.command {
        Command::Dvdbnd(args) => dvdbnd::mount_from_args(args),
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use color_eyre::eyre;

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
