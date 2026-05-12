use clap::{Args, Parser, Subcommand};

#[derive(Parser, Debug)]
#[clap(author, version)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
#[clap(after_help = r#"Examples (Linux):
    fsvfs dvdbnd --root /xyz Game/Data1.bdt Game/Data1.bhd Game/Data2.bdt Game/Data2.bhd
    fsvfs dvdbnd --root /xyz --game DarkSouls_PC DATA/dvdbnd0.bdt DATA/dvdbnd0.bhd5 DATA/dvdbnd1.bdt DATA/dvdbnd1.bhd5

Examples (Windows):
    fsvfs dvdbnd --root Z:\ Game\Data1.bdt Game\Data1.bhd Game\Data2.bdt Game\Data2.bhd
    fsvfs dvdbnd --root Z:\ --game DarkSouls_PC DATA\dvdbnd0.bdt DATA\dvdbnd0.bhd5 DATA\dvdbnd1.bdt DATA\dvdbnd1.bhd5
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

    /// Root at which to mount the filesystem (e.g. "/darksouls", or "Z:\").
    ///
    /// Valid roots are OS-specific (and different between Linux and Windows).
    #[arg(short, long)]
    pub root: String,

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
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use crate::cli::{Cli, Command};

    const ROOT: &str = "/darksouls";
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
            "-r",
            ROOT,
            ARCHIVES[0],
            ARCHIVES[1],
            ARCHIVES[2],
            ARCHIVES[3],
        ]);

        #[allow(irrefutable_let_patterns)]
        let Command::Dvdbnd(command) = cli.command else {
            panic!("Wrong variant, expected `Dvdbnd`");
        };

        assert_eq!(command.root, ROOT);
        assert_eq!(command.dict, None);
        assert_eq!(command.keys, None);
        assert_eq!(command.archive, ARCHIVES);
    }

    #[test]
    fn dvdbnd_args_with_dict_keys() {
        let cli = Cli::parse_from([
            "fsvfs",
            "dvdbnd",
            "-r",
            ROOT,
            "-k",
            "ds3-keys",
            "-d",
            "ds3-hashes",
            ARCHIVES[0],
            ARCHIVES[1],
            ARCHIVES[2],
            ARCHIVES[3],
        ]);

        #[allow(irrefutable_let_patterns)]
        let Command::Dvdbnd(command) = cli.command else {
            panic!("Wrong variant, expected `Dvdbnd`");
        };

        assert_eq!(command.root, ROOT);
        assert_ne!(command.dict, None);
        assert_ne!(command.keys, None);
        assert_eq!(command.archive, ARCHIVES);
    }
}
