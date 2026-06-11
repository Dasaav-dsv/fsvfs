use std::num::TryFromIntError;

impl_uints! {
    pub U16(pub [u8; 2]) as u16;
    pub U24(pub [u8; 3]) as u32;
    pub U32(pub [u8; 4]) as u32;
    pub U64(pub [u8; 8]) as u64;
}

macro_rules! impl_uints {
    ($(
        $vis_outer:vis $ty:ident($vis_inner:vis [u8; $b:literal]) as $as:ty;
    )+) => {
        $(
            #[derive(
                Clone,
                Copy,
                Debug,
                PartialEq,
                Eq,
                PartialOrd,
                Ord,
                Hash,
                zerocopy::Immutable,
                zerocopy::Unaligned,
                zerocopy::KnownLayout,
                zerocopy::IntoBytes,
                zerocopy::FromBytes,
                rkyv::Archive,
                rkyv::Serialize,
                rkyv::Deserialize,
                rkyv::Portable,
            )]
            #[repr(transparent)]
            $vis_outer struct $ty($vis_inner [u8; $b]);
            impl $ty {
                pub const MAX: Self = Self::new(<$as>::MAX);
                #[inline]
                pub const fn new(value: $as) -> Self {
                    let src = value.to_ne_bytes();
                    let (first, last) = cfg_select! {
                        target_endian = "big" => (size_of::<$as>() - $b, size_of::<$as>()),
                        target_endian = "little" => (0, $b),
                    };
                    let mut bytes = [0; $b];
                    let mut i = 0;
                    while i < last - first {
                        bytes[i] = src[first + i];
                        i += 1;
                    }
                    Self(bytes)
                }
                #[inline]
                pub const fn get(self) -> $as {
                    let src = self.0;
                    let (first, last) = cfg_select! {
                        target_endian = "big" => (size_of::<$as>() - $b, size_of::<$as>()),
                        target_endian = "little" => (0, $b),
                    };
                    let mut bytes = <$as>::to_ne_bytes(0);
                    let mut i = 0;
                    while i < last - first {
                        bytes[first + i] = src[i];
                        i += 1;
                    }
                    <$as>::from_ne_bytes(bytes)
                }
            }
            impl TryFrom<$as> for $ty {
                type Error = TryFromIntError;
                #[inline]
                fn try_from(value: $as) -> Result<Self, Self::Error> {
                    if value <= const { Self::MAX.get() } {
                        Ok(Self::new(value))
                    } else {
                        Err(u8::try_from(u16::MAX).unwrap_err())
                    }
                }
            }
            impl From<$ty> for $as {
                #[inline]
                fn from(value: $ty) -> Self {
                    value.get()
                }
            }
        )+
    };
}

use impl_uints;
