use std::{collections::BTreeMap, path::PathBuf, rc::Rc, sync::Arc};

use blocking::unblock;
use futures_util::{StreamExt, stream};
use serde::{Deserialize, Serialize};
use steamlocate::locate_all;

#[derive(Clone, Default, Debug, Serialize, Deserialize)]
pub struct LocateConfig(Vec<Rc<Game>>);

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Game {
    pub app_id: u32,
    pub name: String,
    pub bhds: Vec<String>,
}

impl LocateConfig {
    pub async fn locate_games(&self) -> eyre::Result<BTreeMap<PathBuf, Rc<Game>>> {
        let steam_dirs = unblock(locate_all).await?;

        let mut games = BTreeMap::default();

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

            let mut games_iter = stream::iter(&self.0).flat_map_unordered(None, |rc| {
                stream::iter(&libraries)
                    .filter_map(move |library| async move {
                        let library_ = library.clone();
                        let app_id = rc.app_id;

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
