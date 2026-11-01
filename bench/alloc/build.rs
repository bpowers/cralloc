//! Links the baseline chosen by feature from the shared baselines directory that
//! bench/baselines/build.sh fills: bench/baselines/out of the main checkout, or
//! BENCH_BASELINES. Allocators that cargo builds itself (through a crate dependency) need
//! nothing here.
use std::path::{Path, PathBuf};
use std::process::Command;

/// bench/baselines/out of the main checkout (the parent of the git directory every worktree
/// shares), as build.sh and the runner find it.
fn shared_baselines(manifest_dir: &Path) -> PathBuf {
    let common = Command::new("git")
        .arg("-C")
        .arg(manifest_dir)
        .args(["rev-parse", "--path-format=absolute", "--git-common-dir"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| PathBuf::from(s.trim()));
    match common.as_deref().and_then(Path::parent) {
        Some(main) => main.join("bench/baselines/out"),
        None => manifest_dir.join("../baselines/out"),
    }
}

fn main() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let base =
        std::env::var_os("BENCH_BASELINES").map(PathBuf::from).unwrap_or_else(|| shared_baselines(&manifest_dir));
    println!("cargo:rerun-if-env-changed=BENCH_BASELINES");
    // Every feature of this crate names an allocator.
    let chosen: Vec<String> = std::env::vars()
        .filter_map(|(k, _)| k.strip_prefix("CARGO_FEATURE_").map(|f| f.to_lowercase().replace('_', "-")))
        // Measurement options on top of the cralloc feature, not allocators.
        .filter(|f| {
            !["cralloc-memstat", "cralloc-background", "cralloc-domains", "live-bytes", "memacct"].contains(&f.as_str())
        })
        .collect();
    if chosen.len() > 1 {
        panic!("bench-alloc: pick at most one allocator feature, got {chosen:?}");
    }
    println!("cargo::rustc-check-cfg=cfg(bench_system)");
    if chosen.is_empty() {
        println!("cargo:rustc-cfg=bench_system");
    }
    // `benchrun layout`'s padN variants: N bytes of code that never runs, so that the code the
    // linker places after it moves by exactly N (src/pad.rs). Unset or zero: no pad at all,
    // the normal build.
    println!("cargo:rerun-if-env-changed=BENCH_LAYOUT_PAD");
    println!("cargo::rustc-check-cfg=cfg(bench_layout_pad)");
    let pad: usize = std::env::var("BENCH_LAYOUT_PAD")
        .ok()
        .filter(|s| !s.is_empty())
        .map(|s| s.parse().unwrap_or_else(|_| panic!("BENCH_LAYOUT_PAD={s} is not a number of bytes")))
        .unwrap_or(0);
    if pad > 0 {
        println!("cargo:rustc-cfg=bench_layout_pad");
        println!("cargo:rustc-env=BENCH_LAYOUT_PAD_BYTES={pad}");
    }
    let need = |rel: &str| {
        let p = base.join(rel);
        assert!(p.exists(), "{} is missing; run bench/baselines/build.sh", p.display());
        println!("cargo:rerun-if-changed={}", p.display());
        p.parent().unwrap().display().to_string()
    };
    match chosen.first().map(String::as_str) {
        Some("mi") | Some("mi-sized") => {
            println!("cargo:rustc-link-search=native={}", need("mimalloc/libmimalloc.a"));
            println!("cargo:rustc-link-lib=static=mimalloc");
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
