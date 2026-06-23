use std::{
    cell::{OnceCell, RefCell},
    env,
    ffi::OsStr,
    io,
    ops::Deref,
    path::{Path, PathBuf},
    process::Command,
    rc::Rc,
    sync::LazyLock,
};

use async_io::block_on;
use blocking::unblock;
use color_eyre::eyre::{self, Context, OptionExt};
use futures_util::{FutureExt, StreamExt, TryStreamExt, stream, try_join};
use fxhash::FxHashMap;
use rfd::AsyncFileDialog;
use slint::{ComponentHandle, Model, ModelExt, SharedString, spawn_local};

use crate::{
    AppWindow,
    context::AppContext,
    error_popup,
    steam::{Game, LocateConfig},
};

pub struct App {
    window: AppWindow,
    locate_config: OnceCell<LocateConfig>,
    bhds: RefCell<FxHashMap<PathBuf, Rc<Game>>>,
}

impl App {
    #[inline]
    pub fn new() -> eyre::Result<Rc<Self>> {
        let window = AppWindow::new()?;

        let app = Rc::new(Self {
            window,
            locate_config: OnceCell::new(),
            bhds: RefCell::default(),
        });

        app.set_context(AppContext {
            use_cache: true,
            ..Default::default()
        });

        block_on(app.update_bhds())?;

        app.bind(AppWindow::on_menu_opened, App::open);
        app.bind(AppWindow::on_menu_saved, App::save);

        app.bind(AppWindow::on_keys_browsed, App::browse_keys);
        app.bind(AppWindow::on_dict_browsed, App::browse_dict);
        app.bind(AppWindow::on_cache_browsed, App::browse_cache);

        app.bind(AppWindow::on_cache_cleared, App::clear_cache);

        // app.bind(AppWindow::on_game_dir_selected, f);
        // app.bind(AppWindow::on_game_dir_browsed, App::browse_game_dir);

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

        app.bind(AppWindow::on_mount_browsed, App::browse_mount);
        app.bind(AppWindow::on_mounted, App::mount);

        Ok(app)
    }

    fn bind<Fut>(
        self: &Rc<Self>,
        on: impl Fn(&AppWindow, Box<dyn FnMut()>),
        f: impl Fn(Rc<Self>) -> Fut + 'static,
    ) where
        Fut: Future<Output = eyre::Result<()>> + 'static,
    {
        let app = Rc::downgrade(self);

        let f = Box::new(move || {
            let app = app.upgrade().unwrap();
            spawn_local(f(app).map(|res| {
                if let Err(e) = res {
                    error_popup(e);
                }
            }))
            .unwrap();
        });

        on(self, f);
    }

    async fn browse_dir(
        self: Rc<Self>,
        f: impl FnOnce(&AppWindow, SharedString),
    ) -> eyre::Result<()> {
        if let Some(path) = AsyncFileDialog::new().pick_folder().await {
            let path = path
                .inner()
                .to_str()
                .ok_or_eyre("path contains invalid UTF-8")?;

            f(&self, SharedString::from(path));
        }

        Ok(())
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

    async fn update_bhds(&self) -> eyre::Result<()> {
        let locate_config = match self.locate_config.get() {
            Some(config) => config,
            None => {
                let path = Self::app_dir().join("gui/bhds.json");

                let json = async_fs::read_to_string(path).await?;
                let config = serde_json::from_str::<LocateConfig>(&json)?;

                self.locate_config.get_or_init(move || config)
            }
        };

        let bhds = locate_config.locate_games().await?;
        *self.bhds.borrow_mut() = bhds;

        Ok(())
    }

    async fn open(self: Rc<Self>) -> eyre::Result<()> {
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

    async fn save(self: Rc<Self>) -> eyre::Result<()> {
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

    fn browse_keys(self: Rc<Self>) -> impl Future<Output = eyre::Result<()>> {
        self.browse_dir(AppWindow::set_keys_path)
    }

    fn browse_dict(self: Rc<Self>) -> impl Future<Output = eyre::Result<()>> {
        self.browse_dir(AppWindow::set_dict_path)
    }

    fn browse_cache(self: Rc<Self>) -> impl Future<Output = eyre::Result<()>> {
        self.browse_dir(AppWindow::set_cache_path)
    }

    async fn clear_cache(self: Rc<Self>) -> eyre::Result<()> {
        let path = match &*self.get_cache_path() {
            "" => Self::app_dir().join("cache"),
            path => PathBuf::from(path),
        };

        async_fs::read_dir(path)
            .await?
            .try_for_each_concurrent(None, async |entry| -> io::Result<()> {
                let name = PathBuf::from(entry.file_name());

                if name.extension() == Some(OsStr::new("mdb")) {
                    async_fs::remove_file(entry.path()).await?;
                }

                Ok(())
            })
            .await?;

        Ok(())
    }

    async fn browse_game_dir(self: Rc<Self>) -> eyre::Result<()> {
        self.browse_dir(AppWindow::set_current_game_dir).await?;
        Ok(())
    }

    fn browse_mount(self: Rc<Self>) -> impl Future<Output = eyre::Result<()>> {
        self.browse_dir(AppWindow::set_mount_point)
    }

    async fn mount(self: Rc<Self>) -> eyre::Result<()> {
        let context = self.get_context();
        let app_dir = Self::app_dir();

        if context.mount_point == "" {
            return Err(eyre::eyre!(
                "mount point must be an existing, empty directory"
            ));
        }

        if async_fs::read_dir(app_dir.join(context.mount_point.as_str()))
            .await
            .with_context(|| "mount point must be an existing directory")?
            .next()
            .await
            .is_some()
        {
            return Err(eyre::eyre!("mount point must be an empty directory"));
        }

        async fn check_is_dir(dir: &str, app_dir: &Path, name: &'static str) -> eyre::Result<()> {
            if !dir.is_empty()
                && async_fs::metadata(app_dir.join(dir))
                    .await
                    .ok()
                    .is_none_or(|metadata| !metadata.is_dir())
            {
                Err(eyre::eyre!("{name} directory must exist and be accessible"))
            } else {
                Ok(())
            }
        }

        try_join!(
            check_is_dir(&context.keys_path, app_dir, "keys"),
            check_is_dir(&context.dict_path, app_dir, "dictionary"),
            async {
                if context.use_cache {
                    check_is_dir(&context.cache_path, app_dir, "cache").await
                } else {
                    Ok(())
                }
            }
        )?;

        let game_root = Path::new(context.current_game_dir.as_str());

        let bhd_paths = context
            .dvdbnds
            .iter()
            .skip(1)
            .filter_map(|dvdbnd| dvdbnd.checked.then(|| game_root.join(dvdbnd.name.as_str())))
            .collect::<Vec<_>>();

        if bhd_paths.is_empty() {
            return Err(eyre::eyre!("at least one DVDBND must be checked"));
        }

        if stream::iter(&bhd_paths)
            .any(|path| {
                let path = path.clone();
                unblock(move || !path.exists())
            })
            .await
        {
            return Err(eyre::eyre!("all DVDBND paths must exist and be accessible"));
        }

        let fsvfs = app_dir.join(cfg_select! {
            windows => "fsvfs.exe",
            _ => "fsvfs",
        });

        // TODO: pipe errors to stderr.
        let mut command = Command::new(fsvfs);

        command.current_dir(app_dir).arg("dvdbnd");

        if !context.keys_path.is_empty() {
            let keys_dir = app_dir.join(context.keys_path);
            command.args([OsStr::new("-k"), keys_dir.as_os_str()]);
        }

        if !context.dict_path.is_empty() {
            let dict_dir = app_dir.join(context.dict_path);
            command.args([OsStr::new("-d"), dict_dir.as_os_str()]);
        }

        if context.use_cache {
            if !context.cache_path.is_empty() {
                let cache_dir = app_dir.join(context.cache_path);
                command.args([OsStr::new("-c"), cache_dir.as_os_str()]);
            }
        } else {
            command.arg("--no-cache");
        }

        if !context.game_name.is_empty() {
            command.args(["-g", &context.game_name]);
        }

        let mount_point = app_dir.join(context.mount_point.as_str());
        command.args([OsStr::new("-m"), mount_point.as_os_str()]);

        command.args(&bhd_paths);

        command.spawn()?;

        Ok(())
    }
}

impl Deref for App {
    type Target = AppWindow;

    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.window
    }
}
