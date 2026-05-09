use std::path::Path;

pub fn has_bhd_extension<P: AsRef<Path>>(path: P) -> bool {
    path.as_ref()
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("bhd") || ext.eq_ignore_ascii_case("bhd5"))
}

#[cfg(test)]
mod tests {
    use crate::dvdbnd::bhd5::has_bhd_extension;

    #[test]
    fn good_bhd_extension() {
        assert!(has_bhd_extension("data1.bhd"));
        assert!(has_bhd_extension("data1.bhd5"));
        assert!(has_bhd_extension("data1.BHD"));
        assert!(has_bhd_extension("data1.BHD5"));
    }

    #[test]
    fn bad_bhd_extension() {
        assert!(has_bhd_extension("data1.bdt"));
        assert!(has_bhd_extension("data1.bdt5"));
        assert!(has_bhd_extension("data1.xyzbdt"));
        assert!(has_bhd_extension("data1.xyzbdt5"));
    }
}
