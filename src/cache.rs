use std::path::Path;

use heed::{Env, EnvOpenOptions, types::Bytes};
use lz4_flex::{compress_prepend_size, decompress_size_prepended};
use rkyv::{
    Archive, Deserialize, Serialize,
    api::high::{HighSerializer, HighValidator},
    bytecheck::CheckBytes,
    de::Pool,
    from_bytes,
    rancor::{self, Strategy},
    ser::allocator::ArenaHandle,
    to_bytes,
    util::AlignedVec,
};
use tracing::{debug, warn};

#[derive(Clone, Default, Debug)]
pub struct Cache {
    env: Option<Env>,
}

impl Cache {
    pub fn open(path: &Path) -> Self {
        debug!(?path, "opening cache");

        let env = unsafe {
            const MB: usize = 1024 * 1024 * 1024;

            EnvOpenOptions::new()
                .read_txn_with_tls()
                .max_dbs(1)
                .map_size(64 * MB)
                .max_readers(254)
                .open(path)
                .inspect_err(|e| warn!("caching is not available: {e}"))
                .ok()
        };

        Self { env }
    }

    pub fn empty() -> Self {
        debug!("opening empty cache");
        Self { env: None }
    }

    pub fn get<T>(&self, cache_name: &str, key: &[u8]) -> Option<T>
    where
        T: Archive,
        T::Archived: for<'a> CheckBytes<HighValidator<'a, rancor::Error>>,
        T::Archived: Deserialize<T, Strategy<Pool, rancor::Error>>,
    {
        Self::get_in_env(self.env.as_ref()?, cache_name, key)
            .inspect(|value| debug!("got \"{cache_name}\" cache: {}", value.is_some()))
            .inspect_err(|e| warn!("error when getting cache: {e}"))
            .ok()?
    }

    pub fn put<T>(&self, cache_name: &str, key: &[u8], value: &T)
    where
        T: for<'a> Serialize<HighSerializer<AlignedVec, ArenaHandle<'a>, rancor::Error>>,
    {
        if let Some(env) = &self.env
            && let Err(e) = Self::put_in_env(env, cache_name, key, value)
        {
            warn!("error when saving cache: {e}");
        }
    }

    fn get_in_env<T>(env: &Env, cache_name: &str, key: &[u8]) -> eyre::Result<Option<T>>
    where
        T: Archive,
        T::Archived: for<'a> CheckBytes<HighValidator<'a, rancor::Error>>,
        T::Archived: Deserialize<T, Strategy<Pool, rancor::Error>>,
    {
        let rtxn = env.read_txn()?;

        let Some(db) = env.open_database::<Bytes, Bytes>(&rtxn, Some(cache_name))? else {
            return Ok(None);
        };

        let Some(compressed) = db.get(&rtxn, key)? else {
            return Ok(None);
        };

        let bytes = decompress_size_prepended(compressed)?;

        rtxn.commit()?;

        let value = from_bytes::<T, rancor::Error>(&bytes)?;

        Ok(Some(value))
    }

    fn put_in_env<T>(env: &Env, cache_name: &str, key: &[u8], value: &T) -> eyre::Result<()>
    where
        T: for<'a> Serialize<HighSerializer<AlignedVec, ArenaHandle<'a>, rancor::Error>>,
    {
        let mut wtxn = env.write_txn()?;

        let db = env.create_database::<Bytes, Bytes>(&mut wtxn, Some(cache_name))?;

        let bytes = to_bytes::<rancor::Error>(value)?;
        let compressed = compress_prepend_size(&bytes);

        db.put(&mut wtxn, key, &compressed)?;

        wtxn.commit()?;

        Ok(())
    }
}
