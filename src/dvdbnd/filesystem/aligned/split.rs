use std::{
    cell::UnsafeCell,
    ops::{Deref, DerefMut},
    ptr::NonNull,
    rc::Rc,
};

use compio::buf::{IoBufMut, SetLen};

pub struct Split<T> {
    inner: Rc<UnsafeCell<T>>,
    ptr: NonNull<[u8]>,
}

pub trait SplitBuf: Sized {
    type Half;

    fn split(self, index: usize) -> (Split<Self::Half>, Split<Self::Half>);
}

impl<T> SplitBuf for T
where
    T: IoBufMut + DerefMut<Target = [u8]>,
{
    type Half = T;

    #[track_caller]
    fn split(self, index: usize) -> (Split<T>, Split<T>) {
        let rc = Rc::new(UnsafeCell::new(self));
        let slice = unsafe { &mut **rc.get() };

        let (left, right) = slice.split_at_mut(index);

        let left = Split {
            inner: rc.clone(),
            ptr: NonNull::from_mut(left),
        };

        let right = Split {
            inner: rc,
            ptr: NonNull::from_mut(right),
        };

        (left, right)
    }
}

impl<T> SplitBuf for Split<T> {
    type Half = T;

    #[track_caller]
    fn split(mut self, index: usize) -> (Split<T>, Split<T>) {
        let rc = self.inner;
        let slice = unsafe { self.ptr.as_mut() };

        let (left, right) = slice.split_at_mut(index);

        let left = Split {
            inner: rc.clone(),
            ptr: NonNull::from_mut(left),
        };

        let right = Split {
            inner: rc,
            ptr: NonNull::from_mut(right),
        };

        (left, right)
    }
}

impl<T> Deref for Split<T> {
    type Target = [u8];

    #[inline]
    fn deref(&self) -> &Self::Target {
        unsafe { self.ptr.as_ref() }
    }
}

impl<T> DerefMut for Split<T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { self.ptr.as_mut() }
    }
}

impl<T> SetLen for Split<T> {
    unsafe fn set_len(&mut self, len: usize) {
        if len < self.len() {
            self.ptr = NonNull::slice_from_raw_parts(self.ptr.cast(), len);
        }
    }
}
