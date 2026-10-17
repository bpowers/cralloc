//! Quick checks that the built library serves real programs: interposition, a shell pipeline,
//! threads, forks from threads, a process without rseq, and the abort on a foreign pointer.
//! `scripts/preload-check` runs the long list of real programs on demand.
//!
//! The tests run the programs in `tests/programs/`, compiled with the system C compiler, with
//! the library cargo built for this test run preloaded. They run under the test runner's memory
//! cap like every test.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::OnceLock;

/// The library, built once per test run.
///
/// Cargo does not build a `cdylib` for integration tests, and a test build would make it
/// unwind, which a `no_std` library cannot. So the test builds it with the dev profile, into a
/// target directory of its own, so that it never waits on the build lock of the cargo that is
/// running the tests.
fn library() -> PathBuf {
    static LIB: OnceLock<PathBuf> = OnceLock::new();
    LIB.get_or_init(|| {
        let target = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("preload-build");
        let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
        let status = Command::new(cargo)
            .args([
                "build",
                "--quiet",
                "-p",
                "asmalloc-preload",
                "--manifest-path",
            ])
            .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml"))
            .arg("--target-dir")
            .arg(&target)
            .status()
            .expect("cargo runs");
        assert!(status.success(), "building the preload library");
        target.join("debug/libasmalloc_preload.so")
    })
    .clone()
}

/// Compiles `tests/programs/<name>.c` once per run, and returns the executable.
fn program(name: &str) -> PathBuf {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    let dir = DIR.get_or_init(|| {
        let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("preload-programs");
        std::fs::create_dir_all(&dir).expect("a scratch directory");
        dir
    });
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/programs")
        .join(format!("{name}.c"));
    // Each test compiles into its own file, so tests running at once do not race on one.
    let exe = dir.join(format!("{name}-{:?}", std::thread::current().id()));
    let status = Command::new("cc")
        .args(["-O2", "-pthread", "-o"])
        .arg(&exe)
        .arg(&source)
        .status()
        .expect("a C compiler (cc) is installed");
    assert!(status.success(), "compiling {}", source.display());
    exe
}

fn run(preload: bool, command: &mut Command) -> Output {
    if preload {
        command.env("LD_PRELOAD", library());
    } else {
        command.env_remove("LD_PRELOAD");
    }
    command.output().expect("the program starts")
}

fn same_with_and_without(make: impl Fn() -> Command) -> String {
    let plain = run(false, &mut make());
    let preloaded = run(true, &mut make());
    assert!(
        preloaded.status.success(),
        "preloaded run failed: {:?}\n{}",
        preloaded.status,
        String::from_utf8_lossy(&preloaded.stderr)
    );
    assert_eq!(plain.status.code(), preloaded.status.code());
    assert_eq!(
        String::from_utf8_lossy(&plain.stdout),
        String::from_utf8_lossy(&preloaded.stdout)
    );
    String::from_utf8_lossy(&preloaded.stdout).into_owned()
}

#[test]
fn the_library_serves_malloc() {
    let exe = program("interposed");
    let out = run(true, &mut Command::new(&exe));
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "asmalloc");
    let out = run(false, &mut Command::new(&exe));
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "other");
}

#[test]
fn a_shell_pipeline_prints_the_same() {
    let out = same_with_and_without(|| {
        let mut c = Command::new("sh");
        c.args([
            "-c",
            "seq 1 50000 | sort -r | awk '{ s += $1 } END { print s }'; ls -la / | wc -l",
        ]);
        c
    });
    assert!(out.starts_with("1250025000"), "{out}");
}

#[test]
fn threads_hand_blocks_to_each_other() {
    let exe = program("threads");
    let out = same_with_and_without(|| {
        let mut c = Command::new(&exe);
        c.args(["3", "8", "3000"]);
        c
    });
    assert!(out.contains("bad 0"), "{out}");
}

#[test]
fn threads_fork_while_others_allocate() {
    let exe = program("forks");
    let out = same_with_and_without(|| {
        let mut c = Command::new(&exe);
        c.args(["4", "5"]);
        c
    });
    assert_eq!(out.trim(), "forks 20 of 20 children ok");
}

#[test]
fn a_process_without_rseq_is_served() {
    let exe = program("threads");
    let out = same_with_and_without(|| {
        let mut c = Command::new(&exe);
        c.args(["2", "4", "2000"])
            .env("GLIBC_TUNABLES", "glibc.pthread.rseq=0");
        c
    });
    assert!(out.contains("bad 0"), "{out}");
}

#[test]
fn a_pointer_that_is_not_a_block_aborts() {
    use std::os::unix::process::ExitStatusExt;
    let exe = program("foreign");
    for args in [&[][..], &["interior"][..]] {
        let out = run(true, Command::new(&exe).args(args));
        assert_eq!(out.status.signal(), Some(6), "{:?}", out.status);
        assert!(
            String::from_utf8_lossy(&out.stderr).contains("not a block of asmalloc's heap"),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(out.stdout.is_empty());
    }
}
