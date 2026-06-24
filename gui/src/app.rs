use std::{
    cell::{OnceCell, RefCell},
    collections::{BTreeMap, HashMap},
    ffi::OsStr,
    io::{self, PipeReader, Read, pipe},
    ops::Deref,
    path::{Path, PathBuf},
    process::Command,
    rc::Rc,
    sync::Arc,
    thread,
    time::Duration,
};

use async_io::{Timer, block_on};
use blocking::unblock;
use eyre::{Context, OptionExt, Report};
use futures_util::{
    FutureExt, StreamExt, TryStreamExt,
    future::{self, select},
    stream, try_join,
};
use rfd::AsyncFileDialog;
use slint::{ComponentHandle, Model, ModelExt, ModelRc, SharedString, spawn_local};
use xxhash_rust::xxh3::Xxh3DefaultBuilder;

use crate::{
    AppWindow, BhdCheck, GameDirs,
    context::AppContext,
    error_popup,
    steam::{Game, LocateConfig},
};

pub struct App {
    window: AppWindow,
    locate_config: OnceCell<LocateConfig>,
    bhds: RefCell<BTreeMap<PathBuf, Rc<Game>>>,
}

impl App {
    const GAME_DIR_PLACEHOLDER: &str = "(Select or browse a game directory)";

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
            current_game_dir: Self::GAME_DIR_PLACEHOLDER.into(),
            ..Default::default()
        });

        block_on(app.update_bhds())?;

        app.bind(AppWindow::on_menu_opened, App::open);
        app.bind(AppWindow::on_menu_saved, App::save);

        app.bind(AppWindow::on_keys_browsed, App::browse_keys);
        app.bind(AppWindow::on_dict_browsed, App::browse_dict);
        app.bind(AppWindow::on_cache_browsed, App::browse_cache);

        app.bind(AppWindow::on_cache_cleared, App::clear_cache);

        app.bind(AppWindow::on_game_dir_selected, App::select_game_dir);
        app.bind(AppWindow::on_game_dir_browsed, App::browse_game_dir);

        app.bind_on_bhd_checked();

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
                let _ = res.inspect_err(error_popup);
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

    async fn update_bhds(self: &Rc<Self>) -> eyre::Result<()> {
        let locate_config = match self.locate_config.get() {
            Some(config) => config,
            None => {
                let json = async_fs::read_to_string("gui/bhds.json").await?;
                let config = serde_json::from_str::<LocateConfig>(&json)?;

                self.locate_config.get_or_init(move || config)
            }
        };

        let bhds = locate_config.locate_games().await?;

        let dirs = bhds
            .keys()
            .filter_map(|dir| dir.to_str().map(SharedString::from))
            .collect::<Vec<_>>();

        *self.bhds.borrow_mut() = bhds;

        let game_dir = self.get_current_game_dir();
        let active_index = dirs.iter().position(|dir| *dir == game_dir).unwrap_or(0);

        self.set_game_dirs(GameDirs {
            active_index: active_index as i32,
            dirs: ModelRc::from(dirs.as_slice()),
        });

        if let Some(active) = dirs.get(active_index) {
            self.set_current_game_dir(active.clone());
        }

        self.clone().select_game_dir().await?;

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

            self.update_bhds().await?;
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
        let path = self.get_cache_path();
        let path = if path.is_empty() { "cache" } else { &*path };

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

    async fn select_game_dir(self: Rc<Self>) -> eyre::Result<()> {
        let game_dir = self.get_current_game_dir();

        if game_dir == Self::GAME_DIR_PLACEHOLDER {
            return Ok(());
        }

        let game_dir_path = Path::new(&game_dir);

        let game = self
            .bhds
            .borrow()
            .get(game_dir_path)
            .ok_or_eyre("selected game is not on the list?")?
            .clone();

        self.set_game_name(game.name.as_str().into());

        let bhds = self.get_bhds();

        let previously_checked = bhds
            .row_data(0)
            .and_then(|first| {
                (first.parent_dir == game_dir).then(|| {
                    Arc::new(
                        bhds.iter()
                            .map(|bhd| (bhd.name, bhd.checked))
                            .collect::<HashMap<_, _, Xxh3DefaultBuilder>>(),
                    )
                })
            })
            .unwrap_or_default();

        let mut bhds = stream::iter(&game.bhds)
            .filter_map(|bhd| {
                let bhd_path = game_dir_path.join(bhd);
                let parent_dir = game_dir.clone();
                let checked = previously_checked.clone();

                unblock(move || bhd_path.exists() && bhd_path.with_extension("bdt").exists()).map(
                    move |exists| {
                        exists.then(|| {
                            let name = SharedString::from(bhd);
                            let checked = checked.get(&name).cloned().unwrap_or(checked.is_empty());

                            future::ready(BhdCheck {
                                name,
                                parent_dir,
                                checked,
                            })
                        })
                    },
                )
            })
            .buffer_unordered(usize::MAX)
            .collect::<Vec<_>>()
            .await;

        let first = BhdCheck {
            name: slint::format!("({} BHDs)", bhds.len()),
            parent_dir: game_dir,
            checked: bhds.iter().all(|bhd| bhd.checked),
        };

        bhds.insert(0, first);

        self.set_bhds(bhds.as_slice().into());

        Ok(())
    }

    async fn browse_game_dir(self: Rc<Self>) -> eyre::Result<()> {
        try_join!(
            self.clone().browse_dir(AppWindow::set_current_game_dir),
            self.update_bhds(),
        )?;

        Err(eyre::eyre!("not yet implemented"))
    }

    fn bind_on_bhd_checked(self: &Rc<Self>) {
        self.on_bhd_checked({
            let app = self.as_weak();
            move |i| {
                if i != 0 {
                    return;
                }

                let bhds = app.unwrap().get_bhds();
                let checked = bhds.row_data(0).unwrap().checked;

                for i in 1..bhds.row_count() {
                    let mut bhd = bhds.row_data_tracked(i).unwrap();
                    bhd.checked = checked;
                    bhds.set_row_data(i, bhd);
                }
            }
        });
    }

    fn browse_mount(self: Rc<Self>) -> impl Future<Output = eyre::Result<()>> {
        self.browse_dir(AppWindow::set_mount_point)
    }

    async fn mount(self: Rc<Self>) -> eyre::Result<()> {
        let context = self.get_context().check().await?;

        let bhd_paths = context.bhd_paths().await?;

        let (exit_guard, _drop_on_exit) = pipe()?;
        let (stderr_reader, stderr) = pipe()?;

        let mut command = Box::new(Command::new(cfg_select! {
            unix => "./fsvfs",
            windows => ".\\fsvfs.exe",
        }));

        command
            .app_args(&context)
            .arg("--piped")
            .args(bhd_paths)
            .stdin(exit_guard)
            .stderr(stderr);

        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            use windows::Win32::System::Threading::CREATE_NO_WINDOW;
            command.creation_flags(CREATE_NO_WINDOW.0);
        }

        let mut child = unblock(move || command.spawn()).await?;

        let read_stderr = unblock(move || error_popup_from_pipe(stderr_reader));

        let child_wait = unblock(move || {
            child.wait()?;
            Ok(())
        });

        let open_dir = async {
            let mut command = open_dir_command(&context.mount_point);
            Timer::after(Duration::from_millis(200)).await;
            unblock(move || command.spawn()).await
        };

        try_join!(
            select(read_stderr, child_wait).map(|either| either.factor_first().0),
            open_dir
        )?;

        Ok(())
    }
}

impl AppContext {
    async fn check(self) -> eyre::Result<Self> {
        if self.current_game_dir == App::GAME_DIR_PLACEHOLDER {
            return Err(eyre::eyre!("you must select or browse a game directory"));
        }

        if self.mount_point.is_empty() {
            return Err(eyre::eyre!(
                "mount point must be an existing, empty directory"
            ));
        }

        if async_fs::read_dir(&self.mount_point)
            .await
            .wrap_err("mount point must be an existing directory")?
            .next()
            .await
            .is_some()
        {
            return Err(eyre::eyre!("mount point must be an empty directory"));
        }

        async fn check_is_dir(dir: &str, name: &'static str) -> eyre::Result<()> {
            if !dir.is_empty()
                && async_fs::metadata(dir)
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
            check_is_dir(&self.keys_path, "keys"),
            check_is_dir(&self.dict_path, "dictionary"),
            async {
                if self.use_cache {
                    check_is_dir(&self.cache_path, "cache").await
                } else {
                    Ok(())
                }
            }
        )?;

        Ok(self)
    }

    async fn bhd_paths(&self) -> eyre::Result<Vec<String>> {
        let bhd_paths = self
            .bhds
            .iter()
            .skip(1)
            .filter(|&bhd| bhd.checked)
            .map(|bhd| format!("{}/{}", self.current_game_dir.as_str(), bhd.name.as_str()))
            .collect::<Vec<_>>();

        if bhd_paths.is_empty() {
            return Err(eyre::eyre!("at least one BHD must be checked"));
        }

        if stream::iter(&bhd_paths)
            .any(|path| {
                let path = PathBuf::from(path);
                unblock(move || !path.exists())
            })
            .await
        {
            return Err(eyre::eyre!("all BHD paths must exist and be accessible"));
        }

        Ok(bhd_paths)
    }
}

trait CommandExt {
    fn app_args(&mut self, context: &AppContext) -> &mut Self;
}

impl CommandExt for Command {
    fn app_args(&mut self, context: &AppContext) -> &mut Self {
        self.arg("dvdbnd");

        self.args(["-m", &context.mount_point]);

        if !context.keys_path.is_empty() {
            self.args(["-k", &context.keys_path]);
        }

        if !context.dict_path.is_empty() {
            self.args(["-d", &context.dict_path]);
        }

        if context.use_cache {
            if !context.cache_path.is_empty() {
                self.args(["-c", &context.cache_path]);
            }
        } else {
            self.arg("--no-cache");
        }

        if !context.game_name.is_empty() {
            self.args(["-g", &context.game_name]);
        }

        self
    }
}

fn error_popup_from_pipe(mut pipe: PipeReader) -> io::Result<()> {
    let mut output = vec![];

    while pipe.read_to_end(&mut output)? == 0 {
        thread::sleep(Duration::from_millis(1));
    }

    let msg = String::from_utf8_lossy(&output).into_owned();

    error_popup(&Report::msg(msg));

    Ok(())
}

fn open_dir_command<S: AsRef<OsStr>>(path: S) -> Command {
    let mut command = Command::new(cfg_select! {
        windows => "explorer.exe",
        target_os = "macos" => "open",
        unix => "xdg-open",
    });

    command.arg(path);

    command
}

impl Deref for App {
    type Target = AppWindow;

    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.window
    }
}
