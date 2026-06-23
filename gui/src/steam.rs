use std::{collections::BTreeMap, path::PathBuf, rc::Rc, sync::Arc};

use blocking::unblock;
use color_eyre::eyre;
use futures_util::{StreamExt, stream};
use fxhash::FxHashMap;
use serde::{Deserialize, Serialize};
use steamlocate::locate_all;

#[derive(Clone, Default, Debug, Serialize, Deserialize)]
pub struct LocateConfig(BTreeMap<u32, Rc<Game>>);

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Game {
    pub name: String,
    pub bhds: Vec<String>,
}

impl LocateConfig {
    pub async fn locate_games(&self) -> eyre::Result<FxHashMap<PathBuf, Rc<Game>>> {
        let steam_dirs = unblock(locate_all).await?;

        let mut games = FxHashMap::default();

        for steam_dir in steam_dirs {
            let libraries = unblock(move || {
                steam_dir
                    .libraries()
                    .into_iter()
                    .flatten()
                    .flatten()
                    .map(Arc::new)
                    .collect::<Vec<_>>()
            })
            .await;

            let mut games_iter = stream::iter(&self.0).flat_map_unordered(None, |(&app_id, rc)| {
                stream::iter(&libraries)
                    .filter_map(move |library| async move {
                        let library_ = library.clone();
                        let app = unblock(move || library_.app(app_id)).await?.ok()?;

                        Some((library.resolve_app_dir(&app), rc.clone()))
                    })
                    .boxed_local()
            });

            while let Some((path, game)) = games_iter.next().await {
                games.insert(path, game);
            }
        }

        Ok(games)
    }
}
