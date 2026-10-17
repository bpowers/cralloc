fn main() {
    println!("cargo:rerun-if-env-changed=MI_LIB_DIR");
    if std::env::var("CARGO_FEATURE_MI").is_ok() {
        let dir = std::env::var("MI_LIB_DIR").expect("set MI_LIB_DIR to the directory holding libmimalloc.a (see README.md)");
        println!("cargo:rustc-link-search=native={dir}");
        println!("cargo:rustc-link-lib=static=mimalloc");
    }
}
