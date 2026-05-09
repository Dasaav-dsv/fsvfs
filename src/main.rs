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
