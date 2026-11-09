//! Whether the routines link and run in programs too large for a branch to reach across. A
//! conditional branch on aarch64 reaches 1 MiB and takes no veneer from a linker, so what a routine
//! branches to on a miss, and where its slow path may lie, is a question of how big the program
//! is. Each case builds a program of its own (`big_binary/`) and takes a little over a minute, so
//! they run with `scripts/gate --all`.

use std::path::PathBuf;
use std::process::{Command, Output};

/// Builds the program `name` of `big_binary/` and returns where it is.
fn build(name: &str) -> PathBuf {
    let target = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("big-binary");
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let manifest = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/big_binary/Cargo.toml");
    let built = Command::new(cargo)
        .args([
            "build",
            "--release",
            "--bin",
            name,
            "--manifest-path",
            manifest,
            "--target-dir",
        ])
        .arg(&target)
        .output()
        .unwrap();
    assert!(
        built.status.success(),
        "{name} does not build: {}",
        String::from_utf8_lossy(&built.stderr)
    );
    target.join("release").join(name)
}

fn says_ok(output: &Output) -> bool {
    output.status.success() && output.stdout.starts_with(b"ok ")
}

/// 400 MB of text between the routines and their slow paths: each miss takes a long branch, which
/// on aarch64 goes through a veneer the linker made, so the program shows those work.
#[test]
#[ignore = "slow: run with scripts/gate --all"]
fn a_program_of_hundreds_of_megabytes_links_and_runs() {
    let program = build("big_text");
    assert!(std::fs::metadata(&program).unwrap().len() > 400_000_000);
    let output = Command::new(&program).output().unwrap();
    assert!(says_ok(&output), "{output:?}");
    if cfg!(target_arch = "aarch64") {
        let symbols = Command::new("nm").arg(&program).output().unwrap();
        let symbols = String::from_utf8_lossy(&symbols.stdout);
        // The name of a long branch's stub is the linker's: `_veneer` from GNU ld, `Thunk` from lld.
        let stub = |line: &&str| line.contains("_veneer") || line.contains("Thunk");
        assert!(
            symbols
                .lines()
                .filter(stub)
                .any(|line| line.contains("slow")),
            "no slow path lies out of a branch's reach, so this shows nothing"
        );
    }
    // Hundreds of megabytes of one test's scratch are not worth keeping for the next run.
    std::fs::remove_file(&program).unwrap();
}

/// A function of more than a megabyte with the routines inlined in it.
#[test]
#[ignore = "slow: run with scripts/gate --all"]
fn a_function_of_more_than_a_megabyte_compiles_and_runs() {
    let program = build("big_function");
    let output = Command::new(&program).output().unwrap();
    assert!(says_ok(&output), "{output:?}");
}
