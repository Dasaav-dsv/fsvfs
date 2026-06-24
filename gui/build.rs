#[cfg(windows)]
use embed_manifest::{
    manifest::{HeapType, SupportedOS},
    new_manifest,
};

fn main() {
    println!("cargo::rerun-if-changed=ui/*");
    slint_build::compile("ui/app.slint").unwrap();

    #[cfg(windows)]
    embed_manifest::embed_manifest(
        new_manifest("fsvfs-gui")
            .supported_os(SupportedOS::Windows10..)
            .heap_type(HeapType::SegmentHeap),
    )
    .expect("failed to embed manifest")
}
