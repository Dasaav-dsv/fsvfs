use std::{
    cell::{Cell, UnsafeCell},
    io, iter,
    mem::MaybeUninit,
    num::NonZero,
    ops::{Deref, DerefMut},
    pin::Pin,
    ptr::{self, NonNull},
};

use compio::{
    buf::SetLen,
    driver::{BufferAllocator, BufferRef, Proactor, ProactorBuilder},
    fs::File,
    io::AsyncReadManagedAt,
};
use futures_util::{FutureExt, Stream, StreamExt, stream};
use smallvec::{SmallVec, smallvec_inline};

use crate::dvdbnd::filesystem::aligned::split::SplitBuf;

pub mod split;

pub const ALIGNMENT: NonZero<usize> = cfg_select! {
    unix => NonZero::new(64).unwrap(),
    windows => NonZero::new(512).unwrap(),
};

pub const WINDOW_SIZE: usize = ALIGNMENT.get();
pub const BUFFER_LEN: usize = AlignedBufferAllocator::BUFFER_LEN;

pub type AlignedBufferRef = (BufferRef, i64);
pub type AlignedDynBufferRef = (Box<dyn DynAlignedBuf>, i64);

pub trait DynAlignedBuf: Deref<Target = [u8]> + DerefMut {
    fn truncate(&mut self, new_len: usize);
}

impl<B> DynAlignedBuf for B
where
    B: SetLen + Deref<Target = [u8]> + DerefMut,
{
    fn truncate(&mut self, new_len: usize) {
        if self.len() <= new_len {
            return;
        }

        unsafe {
            self.set_len(new_len);
        }
    }
}

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

    stream::iter(body).filter_map(move |(len, pos)| read(file.clone(), len, pos, file_start))
}

pub fn stream_read_windows(
    file: File,
    start: u64,
    len: u32,
    file_start: u64,
    file_len: u32,
) -> Pin<Box<dyn Stream<Item = io::Result<AlignedDynBufferRef>>>> {
    let (end, file_end) = end_bounds(start, len, file_start, file_len);

    let file = match stream_read_one_window(file, start, len, file_start, file_end) {
        Ok(window) => return window,
        Err(file) => file,
    };

    let head = iter::once({
        let pos = start.saturating_sub(WINDOW_SIZE as u64).max(file_start);
        let len = (start - pos) as usize;
        (len, pos)
    });

    let body = (start..end).step_by(BUFFER_LEN).map(move |pos| {
        let len = (end - pos).min(BUFFER_LEN as u64) as usize;
        (len, pos)
    });

    let tail = iter::once({
        let pos = end;
        let end = pos.strict_add(WINDOW_SIZE as u64).min(file_end);
        let len = (end - pos) as usize;
        (len, pos)
    });

    stream::iter(head.chain(body).chain(tail))
        .filter_map(move |(len, pos)| read_dyn(file.clone(), len, pos, file_start))
        .boxed_local()
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

fn stream_read_one_window(
    file: File,
    start: u64,
    len: u32,
    file_start: u64,
    file_end: u64,
) -> Result<Pin<Box<dyn Stream<Item = io::Result<AlignedDynBufferRef>>>>, File> {
    const WINDOW_MAX: usize = BUFFER_LEN - WINDOW_SIZE * 2;

    if len > WINDOW_MAX as u32 {
        return Err(file);
    }

    let Some(pos) = start.checked_sub(WINDOW_SIZE as u64) else {
        return Err(file);
    };

    let end = (start + len as u64)
        .saturating_add(WINDOW_SIZE as u64)
        .min(file_end);

    let stream = async move {
        let res = read(file, (end - pos) as usize, pos, file_start).await;

        let mut context = SmallVec::<[io::Result<AlignedDynBufferRef>; 3]>::new_const();

        if let Some(res) = res {
            match res {
                Ok((buf, head_offset)) => {
                    let (head, rest) = buf.split(WINDOW_SIZE);
                    let (body, tail) = rest.split(len as usize);

                    let body_offset = head_offset + WINDOW_SIZE as i64;
                    let tail_offset = body_offset + len as i64;

                    context = smallvec_inline![
                        Ok((Box::new(head), head_offset)),
                        Ok((Box::new(body), body_offset)),
                        Ok((Box::new(tail), tail_offset)),
                    ];
                }
                Err(e) => context.push(Err(e)),
            }
        }

        stream::iter(context)
    }
    .flatten_stream()
    .boxed_local();

    Ok(stream)
}

async fn read(
    file: File,
    len: usize,
    pos: u64,
    file_start: u64,
) -> Option<io::Result<AlignedBufferRef>> {
    if len == 0 {
        return None;
    }

    let Some(file_offset) = pos.checked_signed_diff(file_start) else {
        return Some(Err(io::Error::from(io::ErrorKind::InvalidInput)));
    };

    match file.read_managed_at(len, pos).await {
        Ok(buf) => buf.map(|buf| {
            if buf.len() >= len {
                Ok((buf, file_offset))
            } else {
                Err(io::ErrorKind::UnexpectedEof.into())
            }
        }),
        Err(e) => Some(Err(e)),
    }
}

fn read_dyn(
    file: File,
    len: usize,
    pos: u64,
    file_start: u64,
) -> impl Future<Output = Option<io::Result<(Box<dyn DynAlignedBuf>, i64)>>> {
    read(file, len, pos, file_start).map(|opt| {
        opt.map(|res| {
            res.map(|(buf, file_offset)| (Box::new(buf) as Box<dyn DynAlignedBuf>, file_offset))
        })
    })
}

struct AlignedBufferAllocator {
    available: Cell<usize>,
    buffers: UnsafeCell<AlignedBuffers<{ Self::ARRAY_LEN }>>,
}

#[repr(C, align(4096))]
struct AlignedBuffers<const N: usize>(MaybeUninit<[u8; N]>);

const _: () = assert!(align_of::<AlignedBuffers<1>>().is_multiple_of(ALIGNMENT.get()));

impl AlignedBufferAllocator {
    const BUFFER_BLOCK_LEN: usize = 4096;
    const BUFFER_BLOCKS: usize = 8;

    const POOL_SIZE: NonZero<usize> = NonZero::new(usize::BITS as usize).unwrap();
    const BUFFER_LEN: usize = Self::BUFFER_BLOCKS * Self::BUFFER_BLOCK_LEN;

    const ARRAY_BLOCKS: usize = Self::POOL_SIZE.get() * Self::BUFFER_BLOCKS;
    const ARRAY_LEN: usize = Self::POOL_SIZE.get() * Self::BUFFER_LEN;

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
