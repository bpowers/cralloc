//! Declares `cfg(kani)`, which Kani sets, so the `unexpected_cfgs` lint accepts it.
//!
//! The workspace lint table cannot carry the declaration for one crate without moving every
//! crate's lints, so it lives here. If the workspace adds `check-cfg = ['cfg(kani)']` to its
//! `unexpected_cfgs` lint, this file can go.
fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    println!("cargo::rustc-check-cfg=cfg(kani)");
}
