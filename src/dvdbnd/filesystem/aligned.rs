use std::{
    cell::{Cell, UnsafeCell},
    io, iter,
    mem::MaybeUninit,
    num::NonZero,
    ptr::{self, NonNull},
};

use compio::{
    driver::{BufferAllocator, BufferRef, Proactor, ProactorBuilder},
    fs::File,
    io::AsyncReadManagedAt,
};
use futures_util::{Stream, StreamExt, stream};

pub const ALIGNMENT: NonZero<usize> = NonZero::new(8192).unwrap();
pub const BUFFER_LEN: usize = AlignedBufferAllocator::BUFFER_LEN;

pub type AlignedBufferRef = (BufferRef, u32);

pub fn proactor_builder() -> ProactorBuilder {
    AlignedBufferAllocator::proactor_builder()
}

pub fn stream_read(
    file: File,
    start: u64,
    len: u32,
    file_start: u64,
    file_len: u32,
) -> impl Stream<Item = io::Result<AlignedBufferRef>> {
    let (end, _) = end_bounds(start, len, file_start, file_len);

    let body = (start..end).step_by(BUFFER_LEN).map(move |pos| {
        let len = (end - pos).min(BUFFER_LEN as u64) as usize;
        (len, pos)
    });

    stream::iter(body).filter_map(move |(len, pos)| {
        let file = file.clone();
        do_read(file, len, pos, file_start)
    })
}

pub fn stream_read_context(
    file: File,
    start: u64,
    len: u32,
    file_start: u64,
    file_len: u32,
) -> impl Stream<Item = io::Result<AlignedBufferRef>> {
    let (end, file_end) = end_bounds(start, len, file_start, file_len);

    if len < (BUFFER_LEN - ALIGNMENT.get()) as u32 {
        // TODO: single chunk read with Slice<BufferRef>
    }

    let head = iter::once({
        let pos = start.saturating_sub(BUFFER_LEN as u64).max(file_start);
        let len = (start - pos) as usize;
        (len, pos)
    });

    let body = (start..end).step_by(BUFFER_LEN).map(move |pos| {
        let len = (end - pos).min(BUFFER_LEN as u64) as usize;
        (len, pos)
    });

    let tail = iter::once({
        let pos = end;
        let end = pos.strict_add(BUFFER_LEN as u64).min(file_end);
        let len = (end - pos) as usize;
        (len, pos)
    });

    stream::iter(head.chain(body).chain(tail)).filter_map(move |(len, pos)| {
        let file = file.clone();
        do_read(file, len, pos, file_start)
    })
}

#[track_caller]
fn end_bounds(start: u64, len: u32, file_start: u64, file_len: u32) -> (u64, u64) {
    let end = start + len as u64;
    let file_end = file_start + file_len as u64;

    assert!(
        file_start <= start,
        "range ({start}..{end}) is invalid in ({file_start}..{file_end})",
    );

    assert!(
        end <= file_end,
        "range ({start}..{end}) is invalid in ({file_start}..{file_end})",
    );

    (end, file_end)
}

async fn do_read(
    file: File,
    len: usize,
    pos: u64,
    file_start: u64,
) -> Option<io::Result<AlignedBufferRef>> {
    if len == 0 {
        return None;
    }

    match file.read_managed_at(len, pos).await {
        Ok(buf) => {
            let file_offset = (pos - file_start) as u32;
            Some(Ok((buf?, file_offset)))
        }
        Err(e) => Some(Err(e)),
    }
}

struct AlignedBufferAllocator {
    available: Cell<usize>,
    buffers: UnsafeCell<AlignedBuffers<{ Self::ARRAY_LEN }>>,
}

#[repr(align(8192))]
struct AlignedBuffers<const N: usize>(MaybeUninit<[u8; N]>);

const _: () = assert!(align_of::<AlignedBuffers<1>>() == ALIGNMENT.get());

impl AlignedBufferAllocator {
    const BUFFER_BLOCKS: usize = 4;

    const POOL_SIZE: NonZero<usize> = NonZero::new(usize::BITS as usize).unwrap();
    const BUFFER_LEN: usize = Self::BUFFER_BLOCKS * ALIGNMENT.get();

    const ARRAY_BLOCKS: usize = Self::POOL_SIZE.get() * Self::BUFFER_BLOCKS;
    const ARRAY_LEN: usize = Self::ARRAY_BLOCKS * ALIGNMENT.get();

    fn new() -> Box<Self> {
        unsafe {
            let mut uninit = Box::<Self>::new_zeroed();

            let ptr = uninit.as_mut_ptr();
            ptr::write(&raw mut (*ptr).available, Cell::new(!0));

            uninit.assume_init()
        }
    }

    fn proactor_builder() -> ProactorBuilder {
        let mut proactor_builder = Proactor::builder();

        proactor_builder
            .buffer_pool_size(Self::POOL_SIZE.try_into().unwrap())
            .buffer_pool_buffer_len(Self::BUFFER_LEN)
            .buffer_pool_allocator::<Self>();

        proactor_builder
    }

    fn enter<T>(f: impl FnOnce(&Self) -> T) -> T {
        thread_local! {
            static INSTANCE: Box<AlignedBufferAllocator> = AlignedBufferAllocator::new();
        }

        INSTANCE.with(|boxed| f(boxed))
    }

    fn alloc(&self) -> Option<NonNull<MaybeUninit<u8>>> {
        let available = self.available.get();

        let mask = available.checked_sub(1)?;
        let new_available = available & mask;

        self.available.set(new_available);

        let buf = self.buf_ptr();
        let index = (available - new_available).trailing_zeros() as usize;

        let ptr = unsafe { buf.add(index) as *mut MaybeUninit<u8> };

        NonNull::new(ptr)
    }

    fn free(&self, ptr: NonNull<MaybeUninit<u8>>) {
        let index = unsafe {
            let ptr = ptr.as_ptr() as *mut MaybeUninit<[u8; Self::BUFFER_LEN]>;

            assert!(ptr.addr().is_multiple_of(ALIGNMENT.get()));

            let buf = self.buf_ptr();
            let buf_end = buf.add(Self::ARRAY_BLOCKS);

            assert!((buf..buf_end).contains(&ptr));

            ptr.offset_from_unsigned(buf)
        };

        self.available.update(|available| available | (1 << index));
    }

    fn buf_ptr(&self) -> *mut MaybeUninit<[u8; Self::BUFFER_LEN]> {
        unsafe { &raw mut (*self.buffers.get()).0 as *mut MaybeUninit<[u8; Self::BUFFER_LEN]> }
    }
}

impl BufferAllocator for AlignedBufferAllocator {
    fn allocate(_len: u32) -> NonNull<MaybeUninit<u8>> {
        assert_eq!(_len as u64, Self::BUFFER_LEN as u64, "length mismatch");

        Self::enter(|alloc| alloc.alloc()).expect("out of buffers")
    }

    unsafe fn deallocate(ptr: NonNull<MaybeUninit<u8>>, _len: u32) {
        assert_eq!(_len as u64, Self::BUFFER_LEN as u64, "length mismatch");

        Self::enter(|alloc| alloc.free(ptr));
    }
}
