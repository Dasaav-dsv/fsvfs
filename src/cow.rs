use std::borrow::Cow;

pub trait CowExt {
    fn to_ascii_lowercase(cow: &mut Self) -> bool;
}

impl CowExt for Cow<'_, str> {
    #[inline]
    fn to_ascii_lowercase(cow: &mut Self) -> bool {
        let had_uppercase = cow.as_bytes().chunks(16).any(|chunk| {
            chunk
                .iter()
                .fold(false, |is, byte| is | byte.is_ascii_uppercase())
        });

        if had_uppercase {
            Cow::to_mut(cow).make_ascii_lowercase();
        }

        had_uppercase
    }
}
