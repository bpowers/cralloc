//! The routines exported under their C names, on musl. There they are generated copies of their
//! own, which no other test runs: the crate's tests call the routines under the crate's names, and
//! the preload library's run on glibc. So this builds a static musl program whose C allocator is
//! cralloc (`musl_exports/`) and runs it. The build takes a while, so it runs with
//! `scripts/gate --all`.

use std::path::PathBuf;
use std::process::Command;

#[test]
#[ignore = "slow: run with scripts/gate --all"]
fn the_exported_routines_serve_a_static_musl_program() {
    let triple = format!("{}-unknown-linux-musl", std::env::consts::ARCH);
    let installed = Command::new("rustup")
        .args(["target", "list", "--installed"])
        .output()
        .map(|out| String::from_utf8_lossy(&out.stdout).contains(&triple));
    if !matches!(installed, Ok(true)) {
        eprintln!("skipped: the {triple} target is not installed (rustup target add {triple})");
        return;
    }
    let target = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("musl-exports");
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let manifest = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/musl_exports/Cargo.toml");
    let built = Command::new(cargo)
        .args([
            "build",
            "--release",
            "--target",
            &triple,
            "--manifest-path",
            manifest,
        ])
        .arg("--target-dir")
        .arg(&target)
        .output()
        .unwrap();
    assert!(
        built.status.success(),
        "the program does not build: {}",
        String::from_utf8_lossy(&built.stderr)
    );
    let program = target.join(&triple).join("release/musl-exports");
    let output = Command::new(&program).output().unwrap();
    assert!(
        output.status.success() && output.stdout.starts_with(b"ok "),
        "{output:?}"
    );
    eprintln!("{}", String::from_utf8_lossy(&output.stdout));
    // The routines themselves, under their C names, not something of the C library's.
    let symbols = Command::new("nm").arg(&program).output().unwrap();
    let symbols = String::from_utf8_lossy(&symbols.stdout);
    for name in ["malloc", "calloc", "free", "free_sized"] {
        assert!(
            symbols
                .lines()
                .any(|line| line.ends_with(&format!(" T {name}"))),
            "{name} is not defined in the program"
        );
    }
}
