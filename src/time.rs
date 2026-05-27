macro_rules! time {
    ($do:expr, |$elapsed:ident| $log:expr$(,)?) => {{
        let now = ::std::time::Instant::now();
        let res = $do;
        let $elapsed = now.elapsed();
        $log;
        res
    }};
}

pub(crate) use time;
