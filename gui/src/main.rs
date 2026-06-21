#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use color_eyre::eyre;
use slint::Model;

slint::include_modules!();

fn main() -> eyre::Result<()> {
    color_eyre::install()?;

    let ui = AppWindow::new()?;

    ui.on_dvdbnd_checked({
        let ui = ui.as_weak();
        move |i| {
            if i != 0 {
                return;
            }

            let dvdbnds = ui.unwrap().get_dvdbnds();
            let checked = dvdbnds.row_data(0).unwrap().checked;

            for i in 1..dvdbnds.row_count() {
                let mut dvdbnd = dvdbnds.row_data_tracked(i).unwrap();
                dvdbnd.checked = checked;
                dvdbnds.set_row_data(i, dvdbnd);
            }
        }
    });

    ui.run()?;

    Ok(())
}

impl DvdbndCheck {
    fn new(name: &str) -> Self {
        Self {
            name: name.into(),
            checked: false,
        }
    }
}
