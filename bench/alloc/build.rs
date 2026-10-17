//! Links the baseline chosen by feature from bench/baselines/out (BENCH_BASELINES overrides
//! the location), which bench/baselines/build.sh fills.
use std::path::PathBuf;

fn main() {
    let base = std::env::var_os("BENCH_BASELINES")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../baselines/out"));
    println!("cargo:rerun-if-env-changed=BENCH_BASELINES");
    let feature = |f: &str| std::env::var_os(format!("CARGO_FEATURE_{}", f.to_uppercase().replace('-', "_"))).is_some();
    let chosen: Vec<&str> = ["mi", "mi-sized", "je", "tc"].into_iter().filter(|f| feature(f)).collect();
    if chosen.len() > 1 {
        panic!("bench-alloc: pick at most one allocator feature, got {chosen:?}");
    }
    let need = |rel: &str| {
        let p = base.join(rel);
        assert!(p.exists(), "{} is missing; run bench/baselines/build.sh", p.display());
        println!("cargo:rerun-if-changed={}", p.display());
        p.parent().unwrap().display().to_string()
    };
    match chosen.first().copied() {
        Some("mi") | Some("mi-sized") => {
            println!("cargo:rustc-link-search=native={}", need("mimalloc/libmimalloc.a"));
            println!("cargo:rustc-link-lib=static=mimalloc");
        }
        Some("je") => {
            // The distribution's shared library, unprefixed: it also replaces malloc for any
            // C code in the process, unlike tikv-jemallocator's statically linked, prefixed
            // copy.
            println!("cargo:rustc-link-search=native={}", need("jemalloc/libjemalloc.so"));
            println!("cargo:rustc-link-lib=dylib=jemalloc");
        }
        Some("tc") => {
            // A static archive of tcmalloc and all of its dependencies (abseil). Like the
            // tcmalloc-better crate, it also defines malloc and operator new for the process.
            println!("cargo:rustc-link-search=native={}", need("tcmalloc/libtcmalloc.a"));
            println!("cargo:rustc-link-lib=static:+whole-archive=tcmalloc");
            println!("cargo:rustc-link-lib=dylib=stdc++");
        }
        _ => {}
    }
}
