use serde::{Deserialize, Serialize};
use slint::{Model, ModelRc, SharedString, VecModel};

use crate::App;

#[derive(Default, Serialize, Deserialize)]
pub struct AppContext {
    pub mount_point: String,
    pub keys_path: String,
    pub dict_path: String,
    pub cache_path: String,
    pub use_cache: bool,
    pub game_dirs: Vec<String>,
    pub game_dir_index: u32,
    pub current_game_dir: String,
    pub game_name: String,
    pub bhds: Vec<BhdCheck>,
}

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct BhdCheck {
    pub name: String,
    pub checked: bool,
}

impl App {
    pub fn get_context(&self) -> AppContext {
        let game_dirs = self.get_game_dirs();

        AppContext {
            mount_point: self.get_mount_point().into(),
            keys_path: self.get_keys_path().into(),
            dict_path: self.get_dict_path().into(),
            cache_path: self.get_cache_path().into(),
            use_cache: self.get_use_cache(),
            game_dirs: game_dirs.dirs.iter().map(String::from).collect(),
            game_dir_index: game_dirs.active_index.max(0).cast_unsigned(),
            current_game_dir: self.get_current_game_dir().into(),
            game_name: self.get_game_name().into(),
            bhds: self
                .get_bhds()
                .iter()
                .map(|bhd| BhdCheck {
                    name: bhd.name.into(),
                    checked: bhd.checked,
                })
                .collect(),
        }
    }

    pub fn set_context(&self, context: AppContext) {
        self.set_mount_point(context.mount_point.into());
        self.set_keys_path(context.keys_path.into());
        self.set_dict_path(context.dict_path.into());
        self.set_cache_path(context.cache_path.into());
        self.set_use_cache(context.use_cache);

        let game_dirs = context
            .game_dirs
            .iter()
            .map(|dir| dir.as_str().into())
            .collect::<VecModel<SharedString>>();

        self.set_game_dirs(crate::GameDirs {
            dirs: ModelRc::new(game_dirs),
            active_index: context.game_dir_index.cast_signed().max(0),
        });

        self.set_current_game_dir(context.current_game_dir.into());
        self.set_game_name(context.game_name.into());

        let bhds = context
            .bhds
            .iter()
            .map(|bhd| crate::BhdCheck {
                name: bhd.name.as_str().into(),
                checked: bhd.checked,
            })
            .collect::<VecModel<_>>();

        self.set_bhds(ModelRc::new(bhds));
    }
}
