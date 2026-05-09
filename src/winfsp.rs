use winfsp::{host::FileSystemHost, service::FileSystemServiceBuilder, winfsp_init_or_die};

mod context;

#[track_caller]
pub fn start() {
    todo!();
    // let init = winfsp_init_or_die();

    // let fsp = FileSystemServiceBuilder::new()
    //     .with_start(|| Ok(FileSystemHost::new((), ())))
    //     .with_stop(|f| todo!())
    //     .build("balls", init)
    //     .unwrap();

    // let _ = fsp.start().join();
}
