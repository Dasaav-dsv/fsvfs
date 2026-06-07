fn main() {
    #[cfg(windows)]
    delayload_projectedfslib();

    println!("cargo::rerun-if-changed=build.rs");
}

#[cfg(windows)]
fn delayload_projectedfslib() {
    let target_env =
        std::env::var("CARGO_CFG_TARGET_ENV").expect("CARGO_CFG_TARGET_ENV must be set");

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
