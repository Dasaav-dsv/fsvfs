use std::{
    env, io,
    ops::Deref,
    path::{Path, PathBuf},
    sync::LazyLock,
};

use color_eyre::eyre::{self, OptionExt};
use futures_util::{FutureExt, TryStreamExt};
use rfd::AsyncFileDialog;
use slint::{ComponentHandle, Model, ModelExt, PlatformError, SharedString, spawn_local};

use crate::{AppWindow, context::AppContext, error_popup};

#[repr(transparent)]
pub struct App(AppWindow);

impl App {
    #[inline]
    pub fn new() -> Result<Self, PlatformError> {
        let app = AppWindow::new().map(Self)?;

        app.set_context(AppContext {
            use_cache: true,
            ..Default::default()
        });

        app.bind(AppWindow::on_menu_open, App::menu_open);
        app.bind(AppWindow::on_menu_save, App::menu_save);

        app.bind(AppWindow::on_keys_browse, App::keys_browse);
        app.bind(AppWindow::on_dict_browse, App::dict_browse);
        app.bind(AppWindow::on_cache_browse, App::cache_browse);

        app.bind(AppWindow::on_cache_clear, App::cache_clear);

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

        app.bind(AppWindow::on_mount_browse, App::mount_browse);

        Ok(app)
    }

    fn app_dir() -> &'static Path {
        static PATH: LazyLock<PathBuf> = LazyLock::new(|| {
            if let Some(mut path) = env::args_os().next().map(PathBuf::from)
                && path.pop()
            {
                path
            } else {
                PathBuf::from(".")
            }
        });

        &PATH
    }

    fn bind<Fut>(
        &self,
        on: impl Fn(&AppWindow, Box<dyn FnMut() + 'static>),
        f: impl Fn(Self) -> Fut + 'static,
    ) where
        Fut: Future<Output = eyre::Result<()>> + 'static,
    {
        let app = self.as_weak();

        let f = Box::new(move || {
            spawn_local(f(Self(app.unwrap())).map(|res| {
                if let Err(e) = res {
                    error_popup(e);
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

    fn keys_browse(self) -> impl Future<Output = eyre::Result<()>> {
        self.dir_browse(AppWindow::set_keys_path)
    }

    fn dict_browse(self) -> impl Future<Output = eyre::Result<()>> {
        self.dir_browse(AppWindow::set_dict_path)
    }

    fn cache_browse(self) -> impl Future<Output = eyre::Result<()>> {
        self.dir_browse(AppWindow::set_cache_path)
    }

    fn mount_browse(self) -> impl Future<Output = eyre::Result<()>> {
        self.dir_browse(AppWindow::set_mount_path)
    }

    async fn cache_clear(self) -> eyre::Result<()> {
        let path = match &*self.get_cache_path() {
            "" => Self::app_dir().join("cache"),
            path => PathBuf::from(path),
        };

        async_fs::read_dir(path)
            .await?
            .try_for_each_concurrent(None, async |entry| -> io::Result<()> {
                let name = PathBuf::from(entry.file_name());

                if name
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("mdb"))
                {
                    async_fs::remove_file(entry.path()).await?;
                }

                Ok(())
            })
            .await?;

        Ok(())
    }

    async fn dir_browse(self, f: impl FnOnce(&AppWindow, SharedString)) -> eyre::Result<()> {
        if let Some(path) = AsyncFileDialog::new().pick_folder().await {
            let path = path
                .inner()
                .to_str()
                .ok_or_eyre("path contains invalid UTF-8")?;

            f(&self, SharedString::from(path));
        }

        Ok(())
    }
}

impl Clone for App {
    #[inline]
    fn clone(&self) -> Self {
        Self(self.0.clone_strong())
    }
}

impl Deref for App {
    type Target = AppWindow;

    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
