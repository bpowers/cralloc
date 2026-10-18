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

/// Compiles `tests/programs/<file>` (C, or C++ for `.cpp`) and returns the executable.
fn program(file: &str) -> PathBuf {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    let dir = DIR.get_or_init(|| {
        let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("preload-programs");
        std::fs::create_dir_all(&dir).expect("a scratch directory");
        dir
    });
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/programs")
        .join(file);
    // Each test compiles into its own file, so tests running at once do not race on one.
    let exe = dir.join(format!("{file}-{:?}", std::thread::current().id()));
    let (compiler, standard) = if file.ends_with(".cpp") {
        ("c++", "-std=c++20")
    } else {
        ("cc", "-std=gnu17")
    };
    let status = Command::new(compiler)
        .args(["-O2", standard, "-pthread", "-o"])
        .arg(&exe)
        .arg(&source)
        .arg("-ldl")
        .status()
        .expect("a C and C++ compiler (cc, c++) is installed");
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
    let exe = program("interposed.c");
    let out = run(true, &mut Command::new(&exe));
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        format!("asmalloc {}", env!("CARGO_PKG_VERSION"))
    );
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
    let exe = program("threads.c");
    let out = same_with_and_without(|| {
        let mut c = Command::new(&exe);
        c.args(["3", "8", "3000"]);
        c
    });
    assert!(out.contains("bad 0"), "{out}");
}

#[test]
fn threads_fork_while_others_allocate() {
    let exe = program("forks.c");
    let out = same_with_and_without(|| {
        let mut c = Command::new(&exe);
        c.args(["4", "5"]);
        c
    });
    assert_eq!(out.trim(), "forks 20 of 20 children ok");
}

#[test]
fn a_process_without_rseq_is_served() {
    let exe = program("threads.c");
    let out = same_with_and_without(|| {
        let mut c = Command::new(&exe);
        c.args(["2", "4", "2000"])
            .env("GLIBC_TUNABLES", "glibc.pthread.rseq=0");
        c
    });
    assert!(out.contains("bad 0"), "{out}");
}

/// An interior pointer of a block, in a region whose class the C frees' copy of the region
/// table names, reaches the generated `free`'s fast path, which checks the region and not the
/// slot boundary (docs/research/zen5-roofline.md, section 14), so it is cached as a block.
#[test]
#[ignore = "decision for the lead: the generated free does not check slot boundaries"]
fn an_interior_pointer_aborts() {
    use std::os::unix::process::ExitStatusExt;
    let exe = program("foreign.c");
    let out = run(true, Command::new(&exe).arg("interior"));
    assert_eq!(out.status.signal(), Some(6), "{:?}", out.status);
}

#[test]
fn a_pointer_outside_the_heap_aborts() {
    use std::os::unix::process::ExitStatusExt;
    let exe = program("foreign.c");
    let out = run(true, &mut Command::new(&exe));
    assert_eq!(out.status.signal(), Some(6), "{:?}", out.status);
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("not a block of asmalloc's heap"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out.stdout.is_empty());
}

/// The four throwing forms of `new`, which the C++ runtime keeps so that it can throw
/// `std::bad_alloc`; the library exports the other 16 forms.
const THROWING_NEW: [&str; 4] = [
    "_Znwm",
    "_Znam",
    "_ZnwmSt11align_val_t",
    "_ZnamSt11align_val_t",
];

#[test]
fn every_cxx_operator_form_meets_its_cases_and_binds_where_it_should() {
    let exe = program("cxx_operators.cpp");
    let out = same_with_and_without(|| Command::new(&exe));
    assert!(!out.contains("FAIL"), "{out}");
    assert_eq!(
        out.lines().filter(|l| l.starts_with("ok ")).count(),
        9,
        "{out}"
    );
    // The dynamic loader's own report of where each operator the program calls was bound.
    let lib = library();
    let traced = run(true, Command::new(&exe).env("LD_DEBUG", "bindings"));
    let report = String::from_utf8_lossy(&traced.stderr);
    let to_lib = format!("to {} ", lib.display());
    let mut ours = std::collections::BTreeSet::new();
    let mut runtime = std::collections::BTreeSet::new();
    for line in report.lines() {
        let Some(at) = line.find("symbol `") else {
            continue;
        };
        let name = &line[at + "symbol `".len()..];
        let name = &name[..name.find('\'').unwrap_or(name.len())];
        if !["_Znw", "_Zna", "_Zdl", "_Zda"]
            .iter()
            .any(|p| name.starts_with(p))
        {
            continue;
        }
        if THROWING_NEW.contains(&name) {
            assert!(line.contains("libstdc++.so"), "{line}");
            runtime.insert(name.to_owned());
        } else {
            assert!(line.contains(&to_lib), "{line}");
            ours.insert(name.to_owned());
        }
    }
    assert_eq!(ours.len(), 16, "{ours:?}");
    assert_eq!(runtime.len(), 4, "{runtime:?}");
}

/// Builds the crate in `tests/programs/<dir>`, a shared library outside the workspace, and
/// returns the build's status and where the library is.
fn build_library(dir: &str, name: &str) -> (std::process::ExitStatus, PathBuf) {
    let target = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("{dir}-build"));
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let output = Command::new(cargo)
        .args(["build", "--quiet", "--manifest-path"])
        .arg(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/programs")
                .join(dir)
                .join("Cargo.toml"),
        )
        .arg("--target-dir")
        .arg(&target)
        .output()
        .expect("cargo runs");
    (output.status, target.join(format!("debug/lib{name}.so")))
}

/// A shared object that holds the allocator must never be unloaded: a thread's `rseq_cs` goes
/// on naming a descriptor inside it, and the kernel reads that at the thread's next
/// preemption. The library is linked `-z nodelete` for that (`build.rs`).
#[test]
fn the_library_is_marked_never_to_be_unloaded() {
    let out = Command::new("readelf")
        .arg("-d")
        .arg(library())
        .output()
        .expect("readelf runs");
    let dynamic = String::from_utf8_lossy(&out.stdout);
    assert!(
        dynamic
            .lines()
            .any(|l| l.contains("(FLAGS_1)") && l.contains("NODELETE")),
        "{dynamic}"
    );
}

/// A C++ program with a malloc of its own. Under the library its `delete` is ours and its
/// throwing `new` is libstdc++'s, which calls the program's malloc: our `delete` hands a
/// pointer outside the heap to the program's `free`.
#[test]
fn a_cxx_program_with_its_own_malloc_runs() {
    let exe = program("own_malloc.cpp");
    let out = same_with_and_without(|| Command::new(&exe));
    assert_eq!(out.trim(), "99500");
}

/// `own_new.cpp` in `mode`, with and without the library: its replaced `new` and `delete` see
/// every block.
fn own_new_runs(mode: &str) {
    let exe = program("own_new.cpp");
    let out = same_with_and_without(|| {
        let mut c = Command::new(&exe);
        c.arg(mode);
        c
    });
    assert_eq!(out.trim(), format!("{mode}: live 0"));
}

/// A program that replaces `operator new(size_t)` and calls the nothrow form, which the
/// library exports: ours must hand off to the C++ runtime's, which calls the program's, or the
/// program's `delete` is given a block its `new` never made.
#[test]
fn a_cxx_program_with_its_own_operator_new_gets_it_from_nothrow_new() {
    own_new_runs("nothrow");
}

/// A program that replaces only `operator new(size_t)` and `operator delete(void*)`, which
/// the standard says is enough. Its blocks sit inside our heap (its `new` is our `malloc` plus
/// a header), so every `delete` form of ours must forward every pointer to the program's.
#[test]
#[ignore = "needs the generated delete routines to read a mirror of their own (lean-c)"]
fn a_cxx_program_with_its_own_operator_new_runs() {
    for mode in ["array", "sized", "matching", "empty"] {
        own_new_runs(mode);
    }
}

/// The C++ export is documented as needing the C export. Nothing enforces it: the crate
/// builds, and at run time a block from libstdc++'s `new` (glibc's malloc) reaches our
/// `delete`, which aborts; one from our nothrow `new` reaches glibc's `free`. The build should
/// fail instead.
#[test]
fn the_cxx_export_without_the_c_export_does_not_build() {
    let (status, library) = build_library("cxx_only", "cxx_only");
    if status.success() {
        let exe = program("cxx.cpp");
        let out = Command::new(&exe)
            .env("LD_PRELOAD", &library)
            .output()
            .expect("the program starts");
        panic!(
            "the crate built; a C++ program run with it: {:?}\n{}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

#[test]
fn a_throwing_new_that_fails_throws_a_catchable_bad_alloc() {
    let exe = program("cxx_operators.cpp");
    let out = same_with_and_without(|| {
        let mut c = Command::new(&exe);
        c.arg("throw");
        c
    });
    assert_eq!(out.trim(), "caught bad_alloc");
}
