use std::borrow::Cow;

pub trait CowExt {
    fn make_ascii_lowercase(cow: &mut Self);
}

impl CowExt for Cow<'_, str> {
    #[inline]
    fn make_ascii_lowercase(cow: &mut Self) {
        if cow.as_bytes().chunks(4).any(|chunk| {
            chunk
                .iter()
                .fold(false, |is, byte| is | byte.is_ascii_uppercase())
        }) {
            Cow::to_mut(cow).make_ascii_lowercase();
        }
    }
}
