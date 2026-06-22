#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use color_eyre::eyre;
use futures_util::FutureExt;
use rfd::{AsyncFileDialog, MessageButtons, MessageDialog, MessageLevel};
use slint::{Model, spawn_local};

use crate::context::AppContext;

mod context;

slint::include_modules!();

fn main() -> eyre::Result<()> {
    color_eyre::install()?;

    let app = App::new()?;

    app.set_context(AppContext {
        use_cache: true,
        ..Default::default()
    });

    app.bind_async(App::on_menu_open, App::menu_open);
    app.bind_async(App::on_menu_save, App::menu_save);

    app.on_dvdbnd_checked({
        let app = app.as_weak();
        move |i| {
            if i != 0 {
                return;
            }

            let dvdbnds = app.unwrap().get_dvdbnds();
            let checked = dvdbnds.row_data(0).unwrap().checked;

            for i in 1..dvdbnds.row_count() {
                let mut dvdbnd = dvdbnds.row_data_tracked(i).unwrap();
                dvdbnd.checked = checked;
                dvdbnds.set_row_data(i, dvdbnd);
            }
        }
    });

    app.run()?;

    Ok(())
}

impl App {
    fn bind_async<Fut>(
        &self,
        on: impl Fn(&Self, Box<dyn FnMut() + 'static>),
        f: impl Fn(Self) -> Fut + 'static,
    ) where
        Fut: Future<Output = eyre::Result<()>> + 'static,
    {
        let app = self.as_weak();

        let f = Box::new(move || {
            spawn_local(f(app.unwrap()).map(|res| {
                if let Err(e) = res {
                    MessageDialog::new()
                        .set_level(MessageLevel::Error)
                        .set_title("Error")
                        .set_description(e.to_string())
                        .set_buttons(MessageButtons::Ok)
                        .show();
                }
            }))
            .unwrap();
        });

        on(self, f);
    }

    async fn menu_open(self) -> eyre::Result<()> {
        if let Some(path) = AsyncFileDialog::new()
            .add_filter("JSON", &["json"])
            .pick_file()
            .await
        {
            let json = async_fs::read_to_string(path.inner()).await?;
            let context = serde_json::from_str(&json)?;

            self.set_context(context);
        }

        Ok(())
    }

    async fn menu_save(self) -> eyre::Result<()> {
        if let Some(path) = AsyncFileDialog::new()
            .add_filter("JSON", &["json"])
            .save_file()
            .await
        {
            let context = self.get_context();
            let json = serde_json::to_string_pretty(&context)?;

            async_fs::write(path.inner(), json.as_str()).await?;
        }

        Ok(())
    }

    fn set_context(&self, context: AppContext) {
        self.set_mount_path(context.mount_path);
        self.set_keys_path(context.keys_path);
        self.set_dict_path(context.dict_path);
        self.set_cache_path(context.cache_path);
        self.set_use_cache(context.use_cache);
        self.set_game_dirs(context.game_dirs);
        self.set_dvdbnds(context.dvdbnds);
    }

    fn get_context(&self) -> AppContext {
        AppContext {
            mount_path: self.get_mount_path(),
            keys_path: self.get_keys_path(),
            dict_path: self.get_dict_path(),
            cache_path: self.get_cache_path(),
            use_cache: self.get_use_cache(),
            game_dirs: self.get_game_dirs(),
            dvdbnds: self.get_dvdbnds(),
        }
    }
}
