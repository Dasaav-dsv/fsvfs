use crate::dvdbnd::hash::mad::{mad_hash, mad_hash32, mad_hash64};

mod mad;

pub fn hash_path32(s: &str) -> Option<u32> {
    let bytes = normalize_suffix_as_bytes(s)?;
    let init = const { mad_hash32(0, b"/") };

    Some(mad_hash::<4, _>(init, bytes))
}

pub fn hash_path64(s: &str) -> Option<u64> {
    let bytes = normalize_suffix_as_bytes(s)?;
    let init = const { mad_hash64(0, b"/") };

    Some(mad_hash::<4, _>(init, bytes))
}

fn normalize_suffix_as_bytes(s: &str) -> Option<&[u8]> {
    let without_root = match s.split_once(':') {
        Some((_, s)) => s,
        None => s,
    };

    let bytes = without_root.as_bytes();
    let (first, rest) = bytes.split_first()?;

    (!rest.is_empty()).then(|| match *first {
        b'/' | b'\\' => rest,
        _ => bytes,
    })
}

#[cfg(test)]
mod tests {
    use crate::dvdbnd::hash::{hash_path32, hash_path64};

    const PATH: &str = "/action/eventnameid.txt";

    #[test]
    fn hash_eq() {
        assert_eq_results(PATH, Some(0xec09d5de), Some(0x800c9074f004323e));
    }

    #[test]
    fn hash_invariant() {
        assert_eq_hashes(PATH, "/action\\EventNameId.TXT");
        assert_eq_hashes(PATH, "\\ACTION/EVENTNAMEID.txt");
    }

    #[test]
    fn hash_normalized() {
        assert_eq_hashes(PATH, "/data3:\\action\\EventNameId.TXT");
        assert_eq_hashes(PATH, "action/eventnameid.txt");
    }

    #[test]
    fn hash_none() {
        assert_eq_results("", None, None);
        assert_eq_results("/data3:", None, None);
        assert_eq_results("/data3:/", None, None);
        assert_eq_results("/data3:\\", None, None);
    }

    #[track_caller]
    fn assert_eq_results(s: &str, hash32: Option<u32>, hash64: Option<u64>) {
        assert_eq!(hash_path32(s), hash32);
        assert_eq!(hash_path64(s), hash64);
    }

    #[track_caller]
    fn assert_eq_hashes(a: &str, b: &str) {
        assert_eq!(hash_path32(a), hash_path32(a));
        assert_eq!(hash_path64(b), hash_path64(b));
    }
}
