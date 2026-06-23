#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use color_eyre::eyre;
use rfd::{MessageButtons, MessageDialog, MessageLevel};

use crate::app::App;

mod app;
mod context;
mod steam;

slint::include_modules!();

fn main() -> eyre::Result<()> {
    color_eyre::install()?;

    let app = App::new()?;
    app.run()?;

    Ok(())
}

fn error_popup<S: ToString>(msg: S) {
    MessageDialog::new()
        .set_level(MessageLevel::Error)
        .set_title("Error")
        .set_description(msg.to_string())
        .set_buttons(MessageButtons::Ok)
        .show();
}
