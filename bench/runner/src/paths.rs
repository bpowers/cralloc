//! Where things live. The workloads are built and run from this checkout; the baseline
//! libraries and the raw results live in the main checkout, shared by every worktree, so
//! that removing a worktree loses neither. `bench/baselines/build.sh` and
//! `bench/alloc/build.rs` find the main checkout the same way.
use std::path::{Path, PathBuf};
use std::process::Command;

pub struct Paths {
    /// `bench/` of this checkout: allocators.toml, the workload builds.
    pub bench: PathBuf,
    /// The built baselines (`BENCH_BASELINES`, or `bench/baselines/out` of the main checkout).
    pub baselines: PathBuf,
    /// Where `run` writes results by default (`BENCH_RESULTS`, or `bench/results` of the main
    /// checkout).
    pub results: PathBuf,
}

impl Paths {
    pub fn new() -> Paths {
        let bench = PathBuf::from(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf();
        let shared = main_checkout(&bench).map(|m| m.join("bench")).unwrap_or_else(|| bench.clone());
        let var = |k: &str, default: PathBuf| std::env::var_os(k).map(PathBuf::from).unwrap_or(default);
        Paths {
            baselines: var("BENCH_BASELINES", shared.join("baselines/out")),
            results: var("BENCH_RESULTS", shared.join("results")),
            bench,
        }
    }
}

/// Holds `.lock` in the baselines directory shared, for as long as the file is open, so
/// that `build.sh`, which holds it exclusively, cannot replace libraries a session or a
/// workload build is using.
pub fn lock_baselines(baselines: &Path) -> Result<std::fs::File, String> {
    use std::os::fd::AsRawFd;
    let path = baselines.join(".lock");
    let f = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .map_err(|e| format!("{}: {e} (run bench/baselines/build.sh)", path.display()))?;
    // SAFETY: flock on a descriptor we own.
    if unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_SH | libc::LOCK_NB) } != 0 {
        return Err(format!(
            "{} is being rebuilt (bench/baselines/build.sh holds {})",
            baselines.display(),
            path.display()
        ));
    }
    Ok(f)
}

/// The main checkout: the parent of the git directory that every worktree shares.
fn main_checkout(dir: &Path) -> Option<PathBuf> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["rev-parse", "--path-format=absolute", "--git-common-dir"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let common = PathBuf::from(String::from_utf8(out.stdout).ok()?.trim());
    common.parent().map(Path::to_path_buf)
}
