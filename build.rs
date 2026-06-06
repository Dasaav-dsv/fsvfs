fn main() {
    let mut build = cc::Build::new();

    #[cfg(feature = "libsais_omp")]
    build
        .define("LIBSAIS_OPENMP", "1")
        .flags(std::env::var("DEP_OPENMP_FLAG").unwrap().split(' '))
        .flag_if_supported("-Wno-deprecated-openmp");

    build
        .include("../libsais/include")
        .file("../libsais/src/libsais.c")
        .compile("libsais");
}
