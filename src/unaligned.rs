impl_ints! {
    pub U16(pub [u8; 2]) as u16;
    pub U24(pub [u8; 3]) as u32;
    pub U32(pub [u8; 4]) as u32;
    pub U64(pub [u8; 8]) as u64;
}

macro_rules! impl_ints {
    ($(
        $vis_outer:vis $ty:ident($vis_inner:vis [u8; $b:literal]) as $as:ty;
    )+) => {
        $(
            #[cfg_attr(
                feature = "rkyv",
                derive(
                    rkyv::Archive,
                    rkyv::Serialize,
                    rkyv::Deserialize,
                    rkyv::Portable,
                )
            )]
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
                zerocopy::FromBytes,
            )]
            #[repr(transparent)]
            $vis_outer struct $ty($vis_inner [u8; $b]);
            impl $ty {
                #[inline]
                const fn new(value: $as) -> Self {
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
                const fn get(self) -> $as {
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
            impl From<$as> for $ty {
                #[inline]
                fn from(value: $as) -> Self {
                    Self::new(value)
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

use impl_ints;
