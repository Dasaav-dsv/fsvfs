use std::sync::Arc;

use color_eyre::eyre;
use parking_lot::{Condvar, Mutex};

pub trait OnInterrupt: Sized {
    type Error: Send + Sync + 'static;

    fn on_interrupt(self) -> Result<(), Self::Error>;
}

pub fn run_until_interrupted<T>(run: T) -> eyre::Result<()>
where
    T: OnInterrupt + Send + Sync + 'static,
    eyre::Error: From<T::Error>,
{
    let lock_cv = Arc::new((Mutex::new(None), Condvar::new()));

    ctrlc::set_handler({
        let lock_cv = lock_cv.clone();
        let mut run = Some(run);
        move || {
            if let Some(run) = run.take() {
                let res = run.on_interrupt();
                let (lock, cvar) = &*lock_cv;

                let mut umount_res = lock.lock();
                *umount_res = Some(res);

                cvar.notify_all();
            }
        }
    })?;

    let (lock, cvar) = &*lock_cv;
    let mut umount_res = lock.lock();
    loop {
        if let Some(res) = umount_res.take() {
            return Ok(res?);
        }

        cvar.wait(&mut umount_res);
    }
}
