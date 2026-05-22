use std::path::Path;

mod byte_order;
mod consts;
pub mod format;
mod magic;

pub use byte_order::ByteOrderExt;

pub fn has_bhd_extension<P: AsRef<Path>>(path: P) -> bool {
    path.as_ref()
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("bhd") || ext.eq_ignore_ascii_case("bhd5"))
}

pub fn strip_bhd_extension(name: &str) -> Option<&str> {
    let name = name.strip_suffix("5").unwrap_or(name);

    let (name, suffix) = name
        .as_bytes()
        .split_at_checked(name.len().wrapping_sub(4))?;

    suffix
        .eq_ignore_ascii_case(b".bhd")
        .then(|| // SAFETY: valid UTF-8 slice before ".bhd"
             unsafe { str::from_utf8_unchecked(name) })
}

#[cfg(test)]
mod tests {
    use crate::dvdbnd::bhd5::{has_bhd_extension, strip_bhd_extension};

    #[test]
    fn good_bhd_extension() {
        assert!(has_bhd_extension("data1.bhd"));
        assert!(has_bhd_extension("data1.bhd5"));
        assert!(has_bhd_extension("data1.BHD"));
        assert!(has_bhd_extension("data1.BHD5"));
    }

    #[test]
    fn bad_bhd_extension() {
        assert!(!has_bhd_extension("data1.bdt"));
        assert!(!has_bhd_extension("data1.bdt5"));
        assert!(!has_bhd_extension("data1.xyzbdt"));
        assert!(!has_bhd_extension("data1.xyzbdt5"));
    }

    #[test]
    fn strip_good_bhd_extension() {
        assert_eq!(strip_bhd_extension("data1.bhd"), Some("data1"));
        assert_eq!(strip_bhd_extension("data1.bhd5"), Some("data1"));
        assert_eq!(strip_bhd_extension("DATA1.BHD"), Some("DATA1"));
        assert_eq!(strip_bhd_extension("DATA1.BHD5"), Some("DATA1"));
    }

    #[test]
    fn strip_bad_bhd_extension() {
        assert_eq!(strip_bhd_extension("data1.bdt"), None);
        assert_eq!(strip_bhd_extension("data1.bdt5"), None);
        assert_eq!(strip_bhd_extension("data1.xyzbdt"), None);
        assert_eq!(strip_bhd_extension("data1.xyzbdt5"), None);
    }
}
