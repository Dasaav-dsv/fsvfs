use clap::{Args, Parser, Subcommand};

#[derive(Parser, Debug)]
#[clap(author, version)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
#[clap(after_help = r#"Examples (Linux):
    fsvfs dvdbnd --mount /run/media/xyz Game/Data1.bhd Game/Data2.bhd
    fsvfs dvdbnd --mount /run/media/xyz --game DarkSouls_PC DATA/dvdbnd0.bhd5 DATA/dvdbnd1.bhd5

Examples (Windows):
    fsvfs dvdbnd --mount Z:\foo\bar Game\Data1.bhd Game\Data2.bhd
    fsvfs dvdbnd --mount Z:\foo\bar --game DarkSouls_PC DATA\dvdbnd0.bhd5 DATA\dvdbnd1.bhd5
"#)]
pub enum Command {
    /// Mount DVDBND archives as a read-only filesystem.
    ///
    /// Only the BHD[5] file paths are expected. BDT paths are ignored,
    /// they are always assumed to be in the same directory as the BHD(s).
    Dvdbnd(DvdbndArgs),
}

#[derive(Args, Debug)]
pub struct DvdbndArgs {
    /// Archives to mount (at least one BHD[5]).
    ///
    /// DVDBND archives are mounted in pairs ("Game/Data1.bhd" and "Game/Data1.bdt",
    /// "Game/Data2.bhd" and "Game/Data2.bdt"), the BDT(s) are expected next to each BHD[5].
    ///
    /// Note that Dark Souls 3 "Data0" is not actually a DVDBND archive,
    /// but an encrypted and compressed regulation file masking as one.
    #[arg(num_args(1..))]
    pub archive: Vec<String>,

    /// Where to mount the filesystem (e.g. "/run/media/darksouls", or "Z:\darksouls").
    ///
    /// The mount point must be an existing, empty directory.
    #[arg(short, long)]
    pub mountpoint: String,

    /// String identifying which game's file name hashes and keys to use.
    ///
    /// This corresponds to the name of the direct parent folder of dictionary or key files,
    /// e.g. "--game DarkSouls_PC" -> "dvdbnd/Hash/DarkSouls_PC/dvdbnd*.txt".
    ///
    /// By default fsvfs tries to match the keys and the names of the archives automatically.
    #[arg(short, long)]
    pub game: Option<String>,

    /// Directory from which to source RSA keys in PEM format for decrypting BHD headers.
    ///
    /// Its subdirectories are treated as separate game versions: "[KEYS]/.../[GAME]/...".
    ///
    /// By default fsvfs uses a keys folder in its install directory.
    #[arg(short, long)]
    pub keys: Option<String>,

    /// Directory from which to source file name dictionaries in text format for hashing.
    ///
    /// Its subdirectories are treated as separate game versions: "[DICT]/.../[GAME]/...".
    ///
    /// By default fsvfs uses a dictionary folder in its install directory.
    #[arg(short, long)]
    pub dict: Option<String>,

    #[command(flatten)]
    pub cache: CacheArgs,

    /// fsvfs internal; stdin acts as an exit guard for the parent process.
    #[arg(long, num_args(0), default_missing_value = "true", hide(true))]
    pub piped: Option<bool>,
}

#[derive(Args, Debug)]
#[group(multiple = false)]
pub struct CacheArgs {
    /// Directory where to store cached artifacts to speed up execution.
    ///
    /// Mutually exclusive with `--no-cache`.
    #[arg(short, long)]
    pub cache: Option<String>,

    /// Do not read or write the cache.
    ///
    /// Mutually exclusive with `-c`, `--cache`.
    #[arg(long("no-cache"), num_args(0), default_missing_value = "true")]
    pub no_cache: Option<bool>,
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use crate::cli::{Cli, Command};

    const MOUNTPOINT: &str = "/darksouls";
    const ARCHIVES: [&str; 4] = [
        "Game/Data1.bdt",
        "Game/Data1.bhd",
        "Game/Data2.bdt",
        "Game/Data2.bhd",
    ];

    #[test]
    fn dvdbnd_args() {
        let cli = Cli::parse_from([
            "fsvfs",
            "dvdbnd",
            "-m",
            MOUNTPOINT,
            ARCHIVES[0],
            ARCHIVES[1],
            ARCHIVES[2],
            ARCHIVES[3],
        ]);

        #[allow(irrefutable_let_patterns)]
        let Command::Dvdbnd(command) = cli.command else {
            panic!("Wrong variant, expected `Dvdbnd`");
        };

        assert_eq!(command.mountpoint, MOUNTPOINT);
        assert_eq!(command.dict, None);
        assert_eq!(command.keys, None);
        assert_eq!(command.cache.cache, None);
        assert_eq!(command.cache.no_cache, None);
        assert_eq!(command.archive, ARCHIVES);
    }

    #[test]
    fn dvdbnd_args_with_dict_keys_cache() {
        let cli = Cli::parse_from([
            "fsvfs",
            "dvdbnd",
            "-m",
            MOUNTPOINT,
            "-k",
            "ds3-keys",
            "-d",
            "ds3-hashes",
            "-c",
            "my-cache",
            ARCHIVES[0],
            ARCHIVES[1],
            ARCHIVES[2],
            ARCHIVES[3],
        ]);

        #[allow(irrefutable_let_patterns)]
        let Command::Dvdbnd(command) = cli.command else {
            panic!("Wrong variant, expected `Dvdbnd`");
        };

        assert_eq!(command.mountpoint, MOUNTPOINT);
        assert_ne!(command.dict, None);
        assert_ne!(command.keys, None);
        assert_ne!(command.cache.cache, None);
        assert_eq!(command.cache.no_cache, None);
        assert_eq!(command.archive, ARCHIVES);
    }
}
