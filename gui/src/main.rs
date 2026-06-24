#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use std::{env, path::PathBuf};

use eyre::OptionExt;
use normpath::PathExt;
use rfd::{MessageButtons, MessageDialog, MessageLevel};

use crate::app::App;

mod app;
mod context;
mod steam;

slint::include_modules!();

fn main() -> eyre::Result<()> {
    unsafe {
        env::set_var("RUST_BACKTRACE", "1");
    }

    run_app().inspect_err(error_popup)
}

fn run_app() -> eyre::Result<()> {
    set_app_dir_as_cwd()?;

    let app = App::new()?;
    app.run()?;

    Ok(())
}

fn error_popup(err: &eyre::Error) {
    MessageDialog::new()
        .set_level(MessageLevel::Error)
        .set_title("Error")
        .set_description(format!("An error was encountered:\n{err:?}"))
        .set_buttons(MessageButtons::Ok)
        .show();
}

fn set_app_dir_as_cwd() -> eyre::Result<()> {
    let app_path = env::args_os()
        .next()
        .map(PathBuf::from)
        .ok_or_eyre("argv[0] is not set?")?
        .normalize()?
        .into_path_buf();

    let app_dir = app_path.parent().ok_or_eyre("path has no parent")?;

    env::set_current_dir(app_dir)?;

    Ok(())
}
