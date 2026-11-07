//! Declares the configurations the crate reads and says whether the real thing exists here.
//!
//! `cralloc_os`: `os::linux` exists, its system calls and the rest of the Linux interface. Set
//! for Linux on an architecture with both, except under Kani, which models the OS.
//!
//! `cralloc_real`: the process's heap exists here as well, on the routines and the OS. Set
//! where `cralloc_os` is, except under Miri, which runs neither assembly nor system calls.
//!
//! Each site asks for one of these instead of repeating the conditions, so that a new target is
//! one line here. Miri and Kani both pass their `cfg` to the query cargo makes for build
//! scripts, which is what `CARGO_CFG_MIRI` and `CARGO_CFG_KANI` come from.
//!
//! `cralloc_a64_direct`, given in `RUSTFLAGS` to a link by `rust-lld`, reads `__rseq_offset` directly
//! on aarch64, to compare that form with the one that stays.
//!
//! `cfg(kani)` is declared here too because the workspace lint table cannot carry the
//! declaration for one crate without moving every crate's lints.
use std::env::var_os;

fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    println!("cargo::rustc-check-cfg=cfg(kani)");
    println!("cargo::rustc-check-cfg=cfg(cralloc_os)");
    println!("cargo::rustc-check-cfg=cfg(cralloc_real)");
    println!("cargo::rustc-check-cfg=cfg(cralloc_a64_direct)");
    let is = |name: &str, value: &str| var_os(name).is_some_and(|v| v == value);
    if is("CARGO_CFG_TARGET_OS", "linux")
        && (is("CARGO_CFG_TARGET_ARCH", "x86_64") || is("CARGO_CFG_TARGET_ARCH", "aarch64"))
        && var_os("CARGO_CFG_KANI").is_none()
    {
        println!("cargo::rustc-cfg=cralloc_os");
        if var_os("CARGO_CFG_MIRI").is_none() {
            println!("cargo::rustc-cfg=cralloc_real");
        }
    }
}
