use std::{
    io::{self, Read},
    sync::{Arc, Condvar, Mutex},
    thread,
};

use color_eyre::eyre;

pub trait OnInterrupt: Sized {
    type Error: Send + Sync + 'static;

    fn interrupt(self) -> Result<(), Self::Error>;
}

pub fn run_until_interrupted<T>(run: T, piped: bool) -> eyre::Result<()>
where
    T: OnInterrupt + Send + Sync + 'static,
    eyre::Error: From<T::Error>,
{
    let lock_cv = Arc::new((Mutex::new(false), Condvar::new()));

    ctrlc::set_handler({
        let lock_cv = lock_cv.clone();
        move || {
            let (lock, cvar) = &*lock_cv;

            let mut interrupted = lock.lock().unwrap();
            *interrupted = true;

            cvar.notify_all();
        }
    })?;

    if piped {
        let lock_cv = lock_cv.clone();
        thread::spawn(move || {
            let res = io::stdin().read_exact(&mut [0]);

            let (lock, cvar) = &*lock_cv;

            // Hold the lock before unwrapping.
            let mut interrupted = lock.lock().unwrap();
            res.expect_err("internally piped stdin is never written");

            *interrupted = true;

            cvar.notify_all();
        });
    }

    let (lock, cvar) = &*lock_cv;
    let mut res = lock.lock().unwrap();
    loop {
        if *res {
            run.interrupt()?;
            break;
        }

        res = cvar.wait(res).unwrap();
    }

    Ok(())
}
