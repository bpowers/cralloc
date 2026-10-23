//! The preload library serving the small programs in `tests/programs/`: interposition, a
//! shell pipeline, threads, forks from threads, signals, `dlopen`, C++ through its runtime, a
//! process without rseq, and the abort on a foreign pointer. The `_fully` tests, ignored, run
//! the heaviest programs longer. `scripts/preload-check` runs real software on demand, and
//! these tests with it, against the release build.
//!
//! The programs are compiled with the system C and C++ compilers, once per test run, and run
//! with the library cargo built for this test run preloaded. They run under the test runner's
//! memory cap like every test.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::{Arc, Mutex, OnceLock};

/// The library, built once per test run, in the profile this test was built in.
///
/// Cargo does not build a `cdylib` for integration tests, and a test build would make it
/// unwind, which a `no_std` library cannot. So the test builds it itself, into a target
/// directory of its own, so that it never waits on the build lock of the cargo that is running
/// the tests.
fn library() -> PathBuf {
    static LIB: OnceLock<PathBuf> = OnceLock::new();
    LIB.get_or_init(|| {
        let target = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("preload-build");
        let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
        let release = !cfg!(debug_assertions);
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
            .args(release.then_some("--release"))
            .status()
            .expect("cargo runs");
        assert!(status.success(), "building the preload library");
        let profile = if release { "release" } else { "debug" };
        target.join(profile).join("libasmalloc_preload.so")
    })
    .clone()
}

/// Compiles `tests/programs/<file>` (C, or C++ for `.cpp`) once per test run, into an
/// executable, or with `shared` into `lib<name>.so`, linked against `link`, and returns it.
/// Tests that need the same program wait for one compile.
fn compile(file: &'static str, shared: bool, link: &[PathBuf]) -> PathBuf {
    static BUILT: Mutex<BTreeMap<&str, Arc<OnceLock<PathBuf>>>> = Mutex::new(BTreeMap::new());
    let once = BUILT
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .entry(file)
        .or_default()
        .clone();
    once.get_or_init(|| {
        let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("preload-programs");
        std::fs::create_dir_all(&dir).expect("a scratch directory");
        let source = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/programs")
            .join(file);
        let stem = file.rsplit_once('.').map_or(file, |(stem, _)| stem);
        let out = dir.join(if shared {
            format!("lib{stem}.so")
        } else {
            stem.to_owned()
        });
        let (compiler, standard) = if file.ends_with(".cpp") {
            ("c++", "-std=c++20")
        } else {
            ("cc", "-std=gnu17")
        };
        // Into a file of this process's own, then renamed, so that a test run elsewhere in
        // this checkout never runs a half-written program.
        let partial = out.with_extension(format!("{}.partial", std::process::id()));
        let status = Command::new(compiler)
            .args(["-O2", standard, "-pthread"])
            .args(shared.then_some(["-shared", "-fPIC"]).into_iter().flatten())
            .arg("-o")
            .arg(&partial)
            .arg(&source)
            .args(link)
            .arg("-ldl")
            .status()
            .expect("a C and C++ compiler (cc, c++) is installed");
        assert!(status.success(), "compiling {}", source.display());
        std::fs::rename(&partial, &out).expect("renaming the compiled program");
        out
    })
    .clone()
}

/// The executable for `tests/programs/<file>`.
fn program(file: &'static str) -> PathBuf {
    compile(file, false, &[])
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

/// The program's output, after checking it is the same with the library preloaded as without.
fn same_with_and_without(make: impl Fn() -> Command) -> String {
    let plain = run_with(None, &mut make());
    let preloaded = run_with(Some(&library()), &mut make());
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
        c.args(["8", "40"]);
        c
    });
    assert_eq!(out.trim(), "forks 320 of 320 children ok");
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
    // checks. A pointer into the middle of a small block is not promised to abort a free, but
    // it is a `realloc`, which checks every pointer before it moves one.
    for args in [
        &[] as &[&str],
        &["interior"],
        &["realloc-interior"],
        &["realloc-outside"],
    ] {
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
/// its `malloc`, under the preload library.
#[test]
fn a_cxx_program_with_its_own_malloc_runs() {
    let exe = program("own_malloc.cpp");
    let out = same_with_and_without(|| Command::new(&exe));
    assert_eq!(out.trim(), "99500");
}

/// A program that replaces only `operator new(size_t)` and `operator delete(void*)`, which the
/// standard says is enough. Its blocks sit inside our heap (its `new` is our `malloc` plus a
/// header). Under the preload library every other form is the C++ runtime's, which forwards to
/// the program's.
#[test]
fn a_cxx_program_with_its_own_operator_new_runs() {
    let exe = program("own_new.cpp");
    for mode in ["array", "sized", "matching", "empty", "nothrow"] {
        let out = same_with_and_without(|| {
            let mut c = Command::new(&exe);
            c.arg(mode);
            c
        });
        assert_eq!(out.trim(), format!("{mode}: live 0"));
    }
}

/// A program that replaces all four basic operators and tells scalar blocks from array blocks
/// (a checker for `new`/`delete[]` mismatches does). The standard's defaults send the array
/// forms to `operator new[]` and `operator delete[]`. With no operator exported, the program
/// gets the runtime's defaults.
#[test]
fn a_program_with_its_own_array_operators_gets_them_from_the_array_forms() {
    let exe = program("tagged_new.cpp");
    let out = same_with_and_without(|| Command::new(&exe));
    assert_eq!(out.trim(), "mismatches 0");
}

/// The operators are replaced by a shared library the program links (as `libtcmalloc.so` and
/// Boehm GC's `libgccpp` do), not by the program itself. The preloaded library comes before it
/// in the search order, so if the preload exported `operator delete` but not the throwing
/// `operator new`, the program's `new` would be the library's and its `delete` ours, which is
/// why the preload library exports no C++ operator.
#[test]
fn a_program_linked_to_a_library_that_replaces_the_operators_runs() {
    let library = compile("lib_new.cpp", true, &[]);
    let exe = compile("use_lib_new.cpp", false, &[library]);
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

/// libstdc++'s nothrow `new` returns null when the new-handler throws, with or without the
/// library.
#[test]
fn a_nothrow_new_whose_handler_throws_returns_null() {
    let exe = program("cxx_operators.cpp");
    let out = same_with_and_without(|| {
        let mut c = Command::new(&exe);
        c.arg("throwing-handler");
        c
    });
    assert_eq!(out.trim(), "nothrow new returned null");
}

#[test]
fn cxx_programs_print_the_same() {
    for file in ["cxx.cpp", "cxx_containers.cpp"] {
        let exe = program(file);
        same_with_and_without(|| Command::new(&exe));
    }
}

/// Exit handlers and stdio teardown run while other threads still allocate.
#[test]
fn a_process_exits_while_threads_allocate() {
    let exe = program("threads.c");
    let out = same_with_and_without(|| {
        let mut c = Command::new(&exe);
        c.arg("exit");
        c
    });
    assert!(out.contains("exit while allocating"), "{out}");
}

/// By default the library creates the allocator's thread, with every signal blocked in it, when
/// the program creates its first thread, and a forked child creates its own the same way;
/// `ASMALLOC_BACKGROUND=0` turns that off.
#[test]
fn the_default_thread_comes_with_the_programs_first_thread() {
    let exe = program("background.c");
    for (value, threads) in [(None, 1), (Some("0"), 0)] {
        let mut command = Command::new(&exe);
        command.arg("spawn");
        match value {
            Some(value) => command.env("ASMALLOC_BACKGROUND", value),
            None => command.env_remove("ASMALLOC_BACKGROUND"),
        };
        let out = run(true, &mut command);
        assert!(out.status.success(), "{:?}", out.status);
        assert_eq!(
            String::from_utf8_lossy(&out.stdout),
            format!(
                "other threads {threads}, blocking every signal {threads}\nchild: other threads \
                 {threads}\n"
            ),
            "ASMALLOC_BACKGROUND={value:?}"
        );
    }
}

/// `ASMALLOC_BACKGROUND` starts the allocator's thread before `main`, with every signal blocked
/// in it, for any value but empty and `0`; a program that creates no thread gets none by
/// default; and a forked child has no such thread.
#[test]
fn the_background_thread_starts_when_asked_and_not_in_a_child() {
    let exe = program("background.c");
    for (value, threads) in [
        (None, 0),
        (Some(""), 0),
        (Some("0"), 0),
        (Some("1"), 1),
        (Some("yes"), 1),
    ] {
        let mut command = Command::new(&exe);
        command.arg("threads");
        match value {
            Some(value) => command.env("ASMALLOC_BACKGROUND", value),
            None => command.env_remove("ASMALLOC_BACKGROUND"),
        };
        let out = run(true, &mut command);
        assert!(out.status.success(), "{:?}", out.status);
        assert_eq!(
            String::from_utf8_lossy(&out.stdout),
            format!(
                "other threads {threads}, blocking every signal {threads}\nchild: other threads 0\n"
            ),
            "ASMALLOC_BACKGROUND={value:?}"
        );
    }
}

/// Exit, with its handlers allocating, while the background thread sleeps or is in the middle
/// of `maintain` (just after a burst of frees, its first rounds drain, launder and purge) and
/// other threads allocate.
fn a_process_exits_while_the_background_thread_runs(delays: &[u32]) {
    let exe = program("background.c");
    for delay in delays {
        let out = same_with_and_without(|| {
            let mut c = Command::new(&exe);
            c.args(["exit", &delay.to_string()])
                .env("ASMALLOC_BACKGROUND", "1");
            c
        });
        assert_eq!(out.trim(), "exit while maintaining");
    }
}

#[test]
fn a_process_exits_while_the_background_thread_runs_quickly() {
    a_process_exits_while_the_background_thread_runs(&[0, 600]);
}

#[test]
#[ignore = "slow: run with scripts/gate --all"]
fn a_process_exits_while_the_background_thread_runs_fully() {
    let delays: Vec<u32> = (0..12).map(|i| i * 250).collect();
    a_process_exits_while_the_background_thread_runs(&delays);
}

/// The case the thread is for: after a burst of frees the program's light work stays inside
/// its caches, so no slow path runs, and only the thread returns the free memory.
#[test]
#[ignore = "slow: run with scripts/gate --all"]
fn the_background_thread_returns_a_quiet_programs_memory() {
    a_quiet_program_gets_its_memory_back("idle", false);
}

/// The same with a trickle of slow paths, a few every half second, as a health check makes:
/// their heartbeats do a small part of what a round of `maintain` does, so the thread runs
/// its rounds whatever the heartbeats did.
#[test]
#[ignore = "slow: run with scripts/gate --all"]
fn the_background_thread_returns_a_trickling_programs_memory() {
    a_quiet_program_gets_its_memory_back("trickle", false);
}

/// The default thread, in a program that created a thread and then went quiet, returns what
/// the heap holds over the floor: the dormant thread wakes at a heartbeat over the floor.
#[test]
#[ignore = "slow: run with scripts/gate --all"]
fn the_default_thread_returns_a_quiet_programs_memory() {
    a_quiet_program_gets_its_memory_back("quiet", true);
}

/// Runs `mode` for 12 s without the thread and with it: asked for by the environment, or, if
/// `by_default`, the default thread with the variable unset.
fn a_quiet_program_gets_its_memory_back(mode: &str, by_default: bool) {
    let exe = program("background.c");
    let measure = |on: bool| {
        let mut command = Command::new(&exe);
        command.args([mode, "12"]);
        match (on, by_default) {
            (false, _) => command.env("ASMALLOC_BACKGROUND", "0"),
            (true, false) => command.env("ASMALLOC_BACKGROUND", "1"),
            (true, true) => command.env_remove("ASMALLOC_BACKGROUND"),
        };
        let out = run(true, &mut command);
        assert!(out.status.success(), "{:?}", out.status);
        let text = String::from_utf8_lossy(&out.stdout).into_owned();
        let numbers: Vec<u64> = text
            .split_whitespace()
            .filter_map(|word| word.parse().ok())
            .collect();
        assert_eq!(numbers.len(), 2, "{text}");
        (numbers[0], numbers[1])
    };
    // The clocked pass keeps the floor of the allowance (32 MiB, `ALLOWANCE_FLOOR_HUGE_PAGES`
    // in asmalloc's policy), so the thread takes what is above it, less a few MiB of the
    // program's own.
    const FLOOR_MIB: u64 = 32;
    let (freed, off) = measure(false);
    let (_, on) = measure(true);
    assert!(
        on < FLOOR_MIB + freed / 16,
        "{mode}: freed {freed} MiB, {on} MiB with the thread after 12 s"
    );
    assert!(
        (on - FLOOR_MIB.min(on)) * 8 < off - FLOOR_MIB.min(off),
        "{mode}: {off} MiB without the thread, {on} with it"
    );
}

#[test]
fn a_process_without_rseq_forks_from_threads() {
    let exe = program("forks.c");
    let out = same_with_and_without(|| {
        let mut c = Command::new(&exe);
        c.args(["4", "20"])
            .env("GLIBC_TUNABLES", "glibc.pthread.rseq=0");
        c
    });
    assert_eq!(out.trim(), "forks 80 of 80 children ok");
}

/// Signal handlers that allocate, interrupting threads that allocate (design: "Signals"). C
/// does not allow this, so only the preloaded run is checked.
#[test]
fn signal_handlers_allocate_while_threads_do() {
    let out = run(true, &mut Command::new(program("signals.c")));
    assert!(
        out.status.success(),
        "{:?}\n{}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn a_library_opened_and_closed_repeatedly_allocates() {
    let exe = program("dl.c");
    let library = compile("dlib.c", true, &[]);
    let out = same_with_and_without(|| {
        let mut c = Command::new(&exe);
        c.arg(&library).arg("200");
        c
    });
    assert!(out.starts_with("dlopen rounds 200 "), "{out}");
}

#[test]
#[ignore = "slow: run with scripts/gate --all"]
fn threads_hand_blocks_to_each_other_fully() {
    let exe = program("threads.c");
    let out = same_with_and_without(|| {
        let mut c = Command::new(&exe);
        c.args(["20", "16", "20000"]);
        c
    });
    assert!(out.contains("bad 0"), "{out}");
}

#[test]
#[ignore = "slow: run with scripts/gate --all"]
fn a_process_without_rseq_is_served_fully() {
    let exe = program("threads.c");
    let out = same_with_and_without(|| {
        let mut c = Command::new(&exe);
        c.args(["5", "16", "20000"])
            .env("GLIBC_TUNABLES", "glibc.pthread.rseq=0");
        c
    });
    assert!(out.contains("bad 0"), "{out}");
}

/// A program that frees its burst before it creates its first thread: the thread is created
/// dormant with the heap already over the floor, looks at the level before it first waits, and
/// returns the burst as it does when the thread was created first (`silent`). Review finding 3.
#[test]
#[ignore = "slow: run with scripts/gate --all"]
fn a_burst_freed_before_the_first_thread_goes_back() {
    let exe = program("background.c");
    let end = |mode: &str| {
        let mut command = Command::new(&exe);
        command.args([mode, "12"]).env_remove("ASMALLOC_BACKGROUND");
        let out = run(true, &mut command);
        assert!(out.status.success(), "{:?}", out.status);
        let text = String::from_utf8_lossy(&out.stdout).into_owned();
        text.split_whitespace()
            .filter_map(|word| word.parse::<u64>().ok())
            .next()
            .unwrap()
    };
    let (silent, late) = (end("silent"), end("late"));
    assert!(
        late < silent + 16,
        "thread first {silent} MiB after 12 s; burst first {late} MiB"
    );
}
