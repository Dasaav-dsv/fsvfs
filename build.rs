fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    os::main();
}

#[cfg(windows)]
mod os {
    use std::env;

    use embed_manifest::{
        manifest::{HeapType, SupportedOS},
        new_manifest,
    };

    pub fn main() {
        delayload_projectedfslib();
        embed_manifest();
    }

    fn delayload_projectedfslib() {
        let target_env =
            env::var("CARGO_CFG_TARGET_ENV").expect("CARGO_CFG_TARGET_ENV must be set");

        match target_env.as_str() {
            "msvc" => {
                println!("cargo:rustc-link-lib=dylib=delayimp");
                println!("cargo:rustc-link-arg=/DELAYLOAD:projectedfslib.dll");
            }
            "gnu" => {
                println!("cargo:rustc-link-arg=-Wl,--delayload=projectedfslib.dll");
            }
            env => {
                panic!("unsupported target {env}");
            }
        }
    }

    fn embed_manifest() {
        embed_manifest::embed_manifest(
            new_manifest("fsvfs")
                .supported_os(SupportedOS::Windows10..)
                .heap_type(HeapType::SegmentHeap),
        )
        .expect("failed to embed manifest")
    }
}

#[cfg(unix)]
mod os {
    pub fn main() {}
}
