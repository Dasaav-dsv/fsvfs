use memmap2::MmapRaw;

use crate::dvdbnd::filesystem::BndFilesystem;

pub struct DvdbndMount<F: BndFilesystem> {
    data: Box<[MmapRaw]>,
    locks: Box<[u8]>,
    fs: F,
}
