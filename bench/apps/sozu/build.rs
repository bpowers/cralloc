//! Tells `src/main.rs` where the `sozu` crate's own `src/worker.rs` is, in the sources cargo
//! extracted for that dependency. `src/main.rs` compiles it unchanged as its `worker` module:
//! the library keeps that module private, but the main process starts each worker by running
//! this executable as `<exe> worker ...`. Compiling the file from there keeps Sōzu's code,
//! which has another licence than this repository (see Cargo.toml), out of the repository.
use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=Cargo.lock");
    let worker = worker_file(&locked_sozu_version());
    println!("cargo:rerun-if-changed={}", worker.display());
    println!("cargo:rustc-env=SOZU_WORKER_RS={}", worker.display());
}

/// The version of `sozu` that this build links, so that the worker module cannot fall out of
/// step with the library when the release changes.
fn locked_sozu_version() -> String {
    let lock = std::fs::read_to_string("Cargo.lock").unwrap_or_else(|e| panic!("Cargo.lock: {e}"));
    lock.split_once("name = \"sozu\"\nversion = \"")
        .and_then(|(_, rest)| rest.split_once('"'))
        .map(|(version, _)| version.to_string())
        .expect("no package `sozu` in Cargo.lock")
}

/// `$CARGO_HOME/registry/src/index.crates.io-<hash>/sozu-<version>/src/worker.rs`. The hash
/// changes with cargo's version, so several directories may hold the crate, but a release on
/// crates.io never changes, so any of them will do.
fn worker_file(version: &str) -> PathBuf {
    let home = std::env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cargo")))
        .expect("CARGO_HOME or HOME");
    let sources = home.join("registry/src");
    let file = format!("sozu-{version}/src/worker.rs");
    std::fs::read_dir(&sources)
        .unwrap_or_else(|e| panic!("{}: {e}", sources.display()))
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().starts_with("index.crates.io-"))
        .map(|e| e.path().join(&file))
        .find(|p| p.is_file())
        .unwrap_or_else(|| {
            panic!(
                "no {file} under {}/index.crates.io-*: this build compiles that file from cargo's \
                 sources of the `sozu` dependency, so it needs them from crates.io, not vendored or replaced",
                sources.display()
            )
        })
}
