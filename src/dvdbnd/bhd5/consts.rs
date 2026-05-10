use zerocopy::{Immutable, KnownLayout, TryFromBytes, Unaligned};

macro_rules! enum_constant {
    ($($i:ident = $b:literal$(,)?)+) => {
        $(
            #[derive(Clone, Copy, Debug, Immutable, KnownLayout, Unaligned, TryFromBytes)]
            #[repr(u8)]
            pub enum $i {
                $i = $b,
            }
        )+
    };
}

enum_constant! {
    B = b'B',
    H = b'H',
    D = b'D',
    _5 = b'5',
}

enum_constant! {
    Zero = 0,
    One = 1,
}

enum_constant! {
    LittleEndian = 0xff,
    BigEndian = 0,
}
