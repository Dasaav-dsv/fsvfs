use std::{
    hash::{BuildHasherDefault, Hash, Hasher},
    marker::PhantomData,
    ptr::NonNull,
};

use hashbrown::HashMap;
use rkyv::{
    Archive, Portable, Serialize, SerializeUnsized,
    boxed::{ArchivedBox, BoxResolver},
    bytecheck::CheckBytes,
    hash::FxHasher64,
    rancor::Fallible,
    with::{ArchiveWith, Identity, InlineAsBox, Map, MapKV, SerializeWith, Skip},
};

use crate::filesystem::{
    paths::components::{AsComponents, ComponentStr, Components},
    readonly::{Config, DefaultConfig},
};

#[derive(Archive, Serialize)]
pub struct Paths<'a, C = DefaultConfig> {
    pub(super) inner: RawPaths<'a>,
    pub(super) _marker: PhantomData<C>,
}

#[derive(Archive, Serialize)]
pub(super) struct RawPaths<'a> {
    #[rkyv(with = Map<InlineAsBox>)]
    pub(super) paths_by_inode: Vec<&'a str>,

    #[rkyv(with = MapKV<InlineAsBox, Identity>)]
    pub(super) inodes_by_path:
        HashMap<ComponentStr<'a, DefaultConfig>, u32, BuildHasherDefault<FxHasher64>>,

    #[rkyv(with = Skip)]
    pub(super) str_store: Option<NonNull<str>>,
}

impl<C> ArchivedPaths<'_, C> {
    pub fn path_by_inode(&self, inode: u32) -> Option<&str> {
        let boxed = self.inner.paths_by_inode.get(inode as usize)?;
        Some(&**boxed)
    }

    pub fn inode_by_path(&self, path: &(impl AsComponents + ?Sized)) -> Option<u32>
    where
        C: Config,
    {
        let inode = self
            .inner
            .inodes_by_path
            .get_with(path.as_components::<C>(), |components, key| {
                components == key.as_components()
            })
            .cloned()?;

        Some(inode.into())
    }
}

#[derive(CheckBytes, Portable)]
#[repr(transparent)]
pub struct ArchivedBoxComponentStr<C>(ArchivedBox<str>, PhantomData<C>);

impl<C> ArchivedBoxComponentStr<C>
where
    C: Config,
{
    fn as_components(&self) -> &Components<ArchivedBox<str>, C> {
        self.0.as_components()
    }
}

impl<C> ArchiveWith<ComponentStr<'_, C>> for InlineAsBox {
    type Archived = ArchivedBoxComponentStr<C>;
    type Resolver = BoxResolver;

    fn resolve_with(
        field: &ComponentStr<'_, C>,
        resolver: Self::Resolver,
        out: rkyv::Place<Self::Archived>,
    ) {
        let out = unsafe { out.cast_unchecked::<ArchivedBox<str>>() };
        ArchivedBox::resolve_from_ref(field.1, resolver, out);
    }
}

impl<S, C> SerializeWith<ComponentStr<'_, C>, S> for InlineAsBox
where
    S: Fallible + ?Sized,
    str: SerializeUnsized<S>,
{
    fn serialize_with(
        field: &ComponentStr<'_, C>,
        serializer: &mut S,
    ) -> Result<Self::Resolver, S::Error> {
        ArchivedBox::serialize_from_ref(field.1, serializer)
    }
}

impl<C> PartialEq for ArchivedBoxComponentStr<C>
where
    C: Config,
{
    fn eq(&self, other: &Self) -> bool {
        self.as_components() == other.as_components()
    }
}

impl<C> Eq for ArchivedBoxComponentStr<C> where C: Config {}

impl<C> Hash for ArchivedBoxComponentStr<C>
where
    C: Config,
{
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.as_components().hash(state);
    }
}
