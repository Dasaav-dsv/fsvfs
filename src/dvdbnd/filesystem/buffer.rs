use std::{
    alloc::{Layout, alloc, dealloc, handle_alloc_error},
    mem::MaybeUninit,
    num::NonZero,
    ptr::NonNull,
};

use compio::{
    BufResult,
    buf::{IoBuf, IoBufMut, SetLen},
    io::AsyncReadAtExt,
};

use crate::dvdbnd::filesystem::encryption::{BLOCK_SIZE, Ciphertext};

#[derive(Clone, Debug)]
pub struct AlignedBuffer {
    inner: NonNull<[MaybeUninit<u8>]>,
    init_len: usize,
    file_offset: u32,
    alloc_len: usize,
    alignment: NonZero<usize>,
}

impl AlignedBuffer {
    pub fn new(len: usize, alignment: NonZero<usize>) -> Self {
        assert!(
            alignment.is_power_of_two(),
            "alignment must be a power of 2"
        );

        if len == 0 {
            return Self {
                inner: NonNull::slice_from_raw_parts(NonNull::dangling(), 0),
                init_len: 0,
                file_offset: 0,
                alloc_len: 0,
                alignment,
            };
        }

        let alloc_len = len + (alignment.get() + BLOCK_SIZE) * 2;
        let layout = Layout::from_size_align(alloc_len, alignment.get()).unwrap();

        let Some(ptr) = (unsafe { NonNull::new(alloc(layout)) }) else {
            handle_alloc_error(layout);
        };

        Self {
            inner: NonNull::slice_from_raw_parts(ptr.cast(), len + BLOCK_SIZE),
            init_len: 0,
            file_offset: 0,
            alloc_len,
            alignment,
        }
    }

    pub async fn fill<R: AsyncReadAtExt>(
        mut self,
        r: &R,
        pos: u64,
        file_offset: u32,
    ) -> BufResult<(), Self> {
        self.file_offset = file_offset;
        r.read_exact_at(self, pos).await
    }

    fn as_ptr(&self) -> NonNull<[MaybeUninit<u8>]> {
        if self.file_offset == 0 {
            return self.inner;
        }

        let alignment = self.alignment.get();
        let offset = alignment.saturating_sub(BLOCK_SIZE);

        let len = self.inner.len();
        let ptr = unsafe { self.inner.cast::<MaybeUninit<u8>>().add(offset) };

        NonNull::slice_from_raw_parts(ptr, len + BLOCK_SIZE)
    }
}

impl AsRef<[u8]> for AlignedBuffer {
    fn as_ref(&self) -> &[u8] {
        self.as_init()
    }
}

impl AsMut<[u8]> for AlignedBuffer {
    fn as_mut(&mut self) -> &mut [u8] {
        self.as_mut_slice()
    }
}

impl Ciphertext for AlignedBuffer {
    fn file_offset(&self) -> u32 {
        self.file_offset
    }

    fn body(&mut self) -> &mut [u8] {
        self.as_mut()
    }

    fn head(&self) -> [u8; BLOCK_SIZE] {
        let init = self.as_init();

        let right = init.len().min(BLOCK_SIZE);
        let left = BLOCK_SIZE - right;

        let mut head = [0; BLOCK_SIZE];
        head[left..].copy_from_slice(&init[..right]);

        head
    }

    fn tail(&self) -> [u8; BLOCK_SIZE] {
        let init = self.as_init();

        let left = init.len().min(BLOCK_SIZE);
        let right = init.len() - left;

        let mut head = [0; BLOCK_SIZE];
        head[..left].copy_from_slice(&init[right..]);

        head
    }
}

impl IoBuf for AlignedBuffer {
    fn as_init(&self) -> &[u8] {
        let init_len = self.init_len;
        unsafe { self.as_ptr().as_ref()[..init_len].assume_init_ref() }
    }
}

impl IoBufMut for AlignedBuffer {
    fn as_uninit(&mut self) -> &mut [MaybeUninit<u8>] {
        unsafe { self.as_ptr().as_mut() }
    }
}

impl SetLen for AlignedBuffer {
    unsafe fn set_len(&mut self, len: usize) {
        self.init_len = len;
    }
}

impl Drop for AlignedBuffer {
    fn drop(&mut self) {
        let len = self.alloc_len;

        if len == 0 {
            return;
        }

        let layout = Layout::from_size_align(len, self.alignment.get()).unwrap();

        unsafe {
            dealloc(self.inner.as_ptr() as *mut u8, layout);
        }
    }
}

unsafe impl Send for AlignedBuffer {}

unsafe impl Sync for AlignedBuffer {}
