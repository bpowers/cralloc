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
    run_with(preload.then(library).as_deref(), command)
}

/// Runs `command` with `lib` preloaded, or nothing.
fn run_with(lib: Option<&Path>, command: &mut Command) -> Output {
    match lib {
        Some(lib) => command.env("LD_PRELOAD", lib),
        None => command.env_remove("LD_PRELOAD"),
    };
    command.output().expect("the program starts")
}

fn same_with_and_without(make: impl Fn() -> Command) -> String {
    same_with_and_without_lib(&library(), make)
}

/// The program's output, after checking it is the same with `lib` preloaded as without.
fn same_with_and_without_lib(lib: &Path, make: impl Fn() -> Command) -> String {
    let plain = run_with(None, &mut make());
    let preloaded = run_with(Some(lib), &mut make());
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

#[test]
fn a_pointer_that_is_not_a_block_the_slow_path_checks_aborts() {
    use std::os::unix::process::ExitStatusExt;
    let exe = program("foreign.c");
    // Outside the heap, and into the middle of a large block: both reach the slow path, which
    // checks. A pointer into the middle of a small block is not promised to abort.
    for args in [&[] as &[&str], &["interior"]] {
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

/// The four throwing forms of `new`, which only the C++ runtime can serve today, because they
/// must throw `std::bad_alloc`.
const THROWING_NEW: [&str; 4] = [
    "_Znwm",
    "_Znam",
    "_ZnwmSt11align_val_t",
    "_ZnamSt11align_val_t",
];

/// Where the dynamic loader bound each C++ allocation operator `exe` calls with `lib`
/// preloaded, by its own report: (operator, whether it bound to `lib`), each once.
fn operator_bindings(lib: &Path, exe: &Path) -> std::collections::BTreeMap<String, bool> {
    let traced = run_with(Some(lib), Command::new(exe).env("LD_DEBUG", "bindings"));
    let report = String::from_utf8_lossy(&traced.stderr);
    let to_lib = format!("to {} ", lib.display());
    let mut bound = std::collections::BTreeMap::new();
    for line in report.lines() {
        let Some(at) = line.find("symbol `") else {
            continue;
        };
        let name = &line[at + "symbol `".len()..];
        let name = &name[..name.find('\'').unwrap_or(name.len())];
        if ["_Znw", "_Zna", "_Zdl", "_Zda"]
            .iter()
            .any(|p| name.starts_with(p))
        {
            assert!(
                line.contains(&to_lib) || line.contains("libstdc++.so"),
                "{line}"
            );
            bound.insert(name.to_owned(), line.contains(&to_lib));
        }
    }
    bound
}

/// The library exports no C++ operator (design: "Interfaces", C++): exporting some of them
/// without the throwing `new` forms is unsound when another library defines them. So every
/// operator a C++ program calls is the C++ runtime's, which calls the library's `malloc`.
#[test]
fn every_cxx_operator_form_meets_its_cases_and_binds_to_the_cxx_runtime() {
    let exe = program("cxx_operators.cpp");
    let out = same_with_and_without(|| Command::new(&exe));
    assert!(!out.contains("FAIL"), "{out}");
    assert_eq!(
        out.lines().filter(|l| l.starts_with("ok ")).count(),
        9,
        "{out}"
    );
    let bound = operator_bindings(&library(), &exe);
    assert_eq!(bound.len(), 20, "{bound:?}");
    assert!(bound.values().all(|&ours| !ours), "{bound:?}");
    let out = Command::new("nm")
        .args(["-D", "--defined-only"])
        .arg(library())
        .output()
        .expect("nm runs");
    let exports = String::from_utf8_lossy(&out.stdout);
    assert!(
        !exports
            .lines()
            .any(|l| l.contains(" _Zn") || l.contains(" _Zd")),
        "{exports}"
    );
}

/// The C++ operators as built for when all of them can be exported: a shared library that
/// exports them with the C functions (`tests/programs/cxx_exports`), built once per test run.
fn cxx_exports() -> PathBuf {
    static LIB: OnceLock<PathBuf> = OnceLock::new();
    LIB.get_or_init(|| {
        let (status, lib) = build_library("cxx_exports", "cxx_exports");
        assert!(status.success(), "building the C++ exports");
        lib
    })
    .clone()
}

/// The libraries the C++ cases run with: the preload library, and the C++ exports.
fn cxx_libraries() -> [PathBuf; 2] {
    [library(), cxx_exports()]
}

#[test]
fn the_cxx_exports_meet_every_case_and_bind_where_they_should() {
    let exe = program("cxx_operators.cpp");
    let lib = cxx_exports();
    let out = same_with_and_without_lib(&lib, || Command::new(&exe));
    assert!(!out.contains("FAIL"), "{out}");
    let bound = operator_bindings(&lib, &exe);
    let (ours, runtime): (Vec<_>, Vec<_>) = bound.iter().partition(|&(_, &ours)| ours);
    assert_eq!(ours.len(), 16, "{bound:?}");
    assert_eq!(runtime.len(), 4, "{bound:?}");
    assert!(
        runtime
            .iter()
            .all(|(name, _)| THROWING_NEW.contains(&name.as_str())),
        "{bound:?}"
    );
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

/// A C++ program with a malloc of its own. Its `new` and `delete` are the C++ runtime's, on
/// its `malloc`, under the preload library; with the C++ exports their `delete` hands a
/// pointer outside the heap to the program's `free`.
#[test]
fn a_cxx_program_with_its_own_malloc_runs() {
    let exe = program("own_malloc.cpp");
    for lib in cxx_libraries() {
        let out = same_with_and_without_lib(&lib, || Command::new(&exe));
        assert_eq!(out.trim(), "99500");
    }
}

/// A program that replaces only `operator new(size_t)` and `operator delete(void*)`, which the
/// standard says is enough. Its blocks sit inside our heap (its `new` is our `malloc` plus a
/// header). Under the preload library every other form is the C++ runtime's, which forwards to
/// the program's; the C++ exports must do the same, forward every pointer to the program's
/// `delete` and hand nothrow `new` to the runtime's.
#[test]
fn a_cxx_program_with_its_own_operator_new_runs() {
    let exe = program("own_new.cpp");
    for lib in cxx_libraries() {
        for mode in ["array", "sized", "matching", "empty", "nothrow"] {
            let out = same_with_and_without_lib(&lib, || {
                let mut c = Command::new(&exe);
                c.arg(mode);
                c
            });
            assert_eq!(out.trim(), format!("{mode}: live 0"), "{}", lib.display());
        }
    }
}

/// The soak's finding with Boehm GC's `libgccpp`: a program linked with a shared library that
/// defines the C++ operators. The loader looks that library up after the preload library, so a
/// preload library that exported some operators would take the program's deletes and be handed
/// the other library's blocks.
#[test]
fn a_program_linked_with_a_library_that_defines_the_cxx_operators_runs() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("lib-operators-{:?}", std::thread::current().id()));
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/programs/lib_operators.cpp");
    let compile = |before: &[&str], after: &[&str]| {
        let status = Command::new("c++")
            .current_dir(&dir)
            .args(["-O2", "-std=c++20"])
            .args(before)
            .arg(&source)
            .args(after)
            .status()
            .expect("a C++ compiler (c++) is installed");
        assert!(status.success(), "compiling {}", source.display());
    };
    compile(&["-shared", "-fPIC", "-o", "liboperators.so"], &[]);
    compile(
        &["-DPROGRAM", "-o", "program"],
        &["-L.", "-loperators", "-Wl,-rpath,$ORIGIN"],
    );
    let out = same_with_and_without(|| Command::new(dir.join("program")));
    assert_eq!(out.trim(), "live 0");
}

/// The C++ export is documented as needing the C export, and the macro makes that a build
/// error.
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

// The two tests below are findings of the review of the replacement rules: each states what
// should happen and fails today. `cargo test -p asmalloc-preload -- --ignored` runs them.

/// A program that replaces all four basic operators and tells scalar blocks from array blocks
/// (a checker for `new`/`delete[]` mismatches does). The standard's defaults send the array
/// forms to `operator new[]` and `operator delete[]`; ours send them to the scalar pair, so the
/// program sees its array blocks deleted as scalars and its nothrow arrays made as scalars.
#[test]
#[ignore = "finding: the array forms forward to a program's scalar operators"]
fn a_program_with_its_own_array_operators_gets_them_from_the_array_forms() {
    let exe = program("tagged_new.cpp");
    let out = same_with_and_without(|| Command::new(&exe));
    assert_eq!(out.trim(), "mismatches 0");
}

/// The operators are replaced by a shared library the program links (as `libtcmalloc.so` does),
/// not by the program itself. The preloaded library comes before it in the search order, so
/// `operator delete` is ours while `operator new`, which we do not define, is the library's:
/// the first `delete` is handed a pointer of the library's and aborts. Rare: it takes a second
/// allocator linked as a shared library under the preload.
#[test]
#[ignore = "finding: operators replaced by a shared library pair its new with our delete"]
fn a_program_linked_to_a_library_that_replaces_the_operators_runs() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("lib-new");
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    let programs = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/programs");
    let library = dir.join("liblib_new.so");
    let status = Command::new("c++")
        .args(["-O2", "-std=c++20", "-shared", "-fPIC", "-o"])
        .arg(&library)
        .arg(programs.join("lib_new.cpp"))
        .status()
        .expect("a C++ compiler");
    assert!(status.success());
    let exe = dir.join("use_lib_new");
    let status = Command::new("c++")
        .args(["-O2", "-std=c++20", "-o"])
        .arg(&exe)
        .arg(programs.join("use_lib_new.cpp"))
        .arg(&library)
        .status()
        .expect("a C++ compiler");
    assert!(status.success());
    let out = same_with_and_without(|| Command::new(&exe));
    assert_eq!(out.trim(), "99500");
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
