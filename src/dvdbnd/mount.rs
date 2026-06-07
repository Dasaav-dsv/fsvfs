use std::{ffi::OsStr, fs, path::Path};

use color_eyre::eyre;
use compio::{dispatcher::Dispatcher, fs::File, runtime::Runtime};
use futures_util::{StreamExt, TryStreamExt, stream};
use rayon::{
    ThreadPoolBuilder,
    iter::{IntoParallelRefIterator, ParallelIterator},
};
use tracing::info;

use crate::{
    dvdbnd::{
        bhd5::FileAny,
        dict::Dictionary,
        filesystem::{DvdbndFile, DvdbndFilesystem, DvdbndRofs, DvdbndRofsBuilder},
        keys::Keys,
    },
    time::time,
};

#[cfg(unix)]
mod fuser;
#[cfg(windows)]
mod projfs;

#[derive(Debug)]
pub struct DvdbndMount<F: DvdbndFilesystem> {
    fs: F,
    bdts: Box<[Bdt]>,
    dispatcher: Dispatcher,
    #[cfg(unix)]
    timestamp: SystemTime,
}

#[derive(Debug)]
struct Bdt {
    file: File,
}

impl DvdbndMount<DvdbndRofs> {
    pub fn from_keys_and_dict(keys: &Keys<'_>, dict: Option<&Dictionary>) -> eyre::Result<Self> {
        let thread_pool = ThreadPoolBuilder::new().use_current_thread().build()?;
        let files = time!(
            thread_pool.in_place_scope_fifo(|_| {
                keys.by_path
                    .par_iter()
                    .map(|(path, key)| {
                        let mut bytes = fs::read(path)?;
                        if let Some(key) = key {
                            let len = key.decrypt_blocks_in_place(&mut bytes)?;
                            bytes.truncate(len);
                        }
                        Ok(bytes)
                    })
                    .collect::<eyre::Result<Vec<_>>>()
            })?,
            |t| info!("decrypted BHD5 files ({t:.02?})"),
        );

        let bhds = time!(
            keys.by_path
                .iter()
                .zip(&files)
                .map(|((path, _), bytes)| {
                    let file = FileAny::try_ref_from_bytes(bytes)?;

                    // FIXME
                    let name = path
                        .file_prefix()
                        .and_then(OsStr::to_str)
                        .unwrap_or_default();

                    Ok((name, file))
                })
                .collect::<eyre::Result<Vec<_>>>()?,
            |t| info!("parsed BHD5 files ({t:.02?})")
        );

        let fs = time!(
            DvdbndRofsBuilder::new()
                .with_bhds(bhds)
                .with_dict(dict)
                .finish()?,
            |t| info!("built dvdbnd read-only filesystem ({t:.02?})"),
        );

        let bdts = keys.by_path.keys().map(|path| path.to_bdt());

        let mount = Runtime::new()?.block_on(Self::from_fs_and_bdts(fs, bdts))?;

        Ok(mount)
    }
}

impl<F: DvdbndFilesystem> DvdbndMount<F> {
    async fn from_fs_and_bdts<P>(fs: F, paths: P) -> eyre::Result<Self>
    where
        P: IntoIterator<Item: AsRef<Path>>,
    {
        let bdts = stream::iter(paths)
            .then(Bdt::open)
            .try_collect::<Vec<_>>()
            .await?;

        let dispatcher = Dispatcher::builder()
            .worker_threads(4.try_into().unwrap())
            .build()?;

        Ok(Self {
            fs,
            bdts: bdts.into_boxed_slice(),
            dispatcher,
            #[cfg(unix)]
            timestamp: std::time::SystemTime::now(),
        })
    }
}

impl Bdt {
    async fn open<P: AsRef<Path>>(path: P) -> eyre::Result<Self> {
        let file = File::open(&path).await?;
        Ok(Self { file })
    }
}
