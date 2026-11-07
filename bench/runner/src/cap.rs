//! Memory caps, by construction.
//!
//! `benchrun run` always runs inside a transient systemd scope of its own with a hard memory
//! limit, swap off, and the cgroup subtree delegated to it; if it is started anywhere else it
//! re-executes itself inside one. Within the scope, every measured process (and every RPC
//! driver) is moved, between `fork` and `execve`, into a fresh child cgroup with its own hard
//! limit. So a workload that runs away is killed by the kernel at its own cap, the runner sees
//! that in the cgroup's `memory.events` and records the run as failed, and the session goes
//! on. The scope's limit is a backstop over everything, the runner included.
//!
//! The scope is `OOMPolicy=continue`: by default systemd stops a whole unit when the kernel
//! kills one of its processes for memory, which would end the session with the first runaway.
//!
//! Every process is already charged to some memory cgroup, so the limits add no work on the
//! allocation path; the charge is compared against a number instead of "max". A run that
//! reaches its limit is different: the kernel reclaims (file pages, since swap is off) before
//! it kills, which slows the run, so such a run is also recorded as failed.
use std::io;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;

/// No cap may exceed this, the session's included.
pub const MAX: u64 = 16 << 30;
/// Each measured process. The largest legitimate peak in the full suite is about 1.2 GiB
/// (churn with 10M live objects; docs/research/workloads.md, "The kernel suite"), so this
/// leaves three times that for an allocator that is worse but working, and fails one that has
/// run away.
pub const DEFAULT_RUN: u64 = 4 << 30;
/// The load generator, which always uses glibc. The open-loop generator keeps a record of
/// every request (20 bytes each: a minute at 450k req/s is over 500 MiB) and its list of
/// commands (hundreds of MiB for the large-heap mini-redis jobs).
pub const DRIVER: u64 = 3 << 30;
/// The runner itself: its samples and results are small.
const RUNNER: u64 = 1 << 30;

/// Set, to the scope's unit name, in the environment of the re-executed runner.
const MARKER: &str = "BENCH_SCOPE";

/// The whole session's limit for a given per-run cap.
pub fn scope_cap(run: u64) -> u64 {
    (run + DRIVER + RUNNER).min(MAX)
}

/// `4G`, `512M`, `1T` (powers of 1024, as systemd and scripts/memlimit read them) or bytes.
pub fn parse_size(s: &str) -> Option<u64> {
    let (digits, shift) = match s.char_indices().last()? {
        (i, 'K') => (&s[..i], 10),
        (i, 'M') => (&s[..i], 20),
        (i, 'G') => (&s[..i], 30),
        (i, 'T') => (&s[..i], 40),
        _ => (s, 0),
    };
    digits.parse::<u64>().ok()?.checked_mul(1 << shift)
}

/// The scope this session runs in, with the runner itself moved into a leaf of it.
pub struct Scope {
    dir: PathBuf,
    pub unit: String,
    /// The scope's own limit, read back from the kernel.
    pub cap: u64,
}

/// Whether this process is the runner re-executed inside its scope by [`enter`].
pub fn in_scope() -> bool {
    std::env::var_os(MARKER).is_some()
}

/// Returns the session's scope, re-executing this process inside a new one first if it is
/// not in one. It never returns without a cap in force: if the scope cannot be made or
/// checked, the error says why and the caller must stop.
pub fn enter(run_cap: u64) -> Result<Scope, String> {
    let Some(unit) = std::env::var_os(MARKER) else {
        return Err(reexec(scope_cap(run_cap)));
    };
    let unit = unit.to_string_lossy().into_owned();
    let dir = own_cgroup().map_err(|e| format!("/proc/self/cgroup: {e}"))?;
    if dir.file_name().is_none_or(|n| n.to_string_lossy() != format!("{unit}.scope")) {
        return Err(format!("{MARKER}={unit} is set, but this process is in {}, not that scope", dir.display()));
    }
    let max = read(&dir.join("memory.max")).map_err(|e| format!("{}: {e}", dir.display()))?;
    let cap = match max.parse::<u64>() {
        Ok(c) if c <= MAX => c,
        _ => return Err(format!("{unit} has memory.max {max}; refusing to run without a cap of at most 16 GiB")),
    };
    // A cgroup with controllers enabled for its children may hold no processes of its own,
    // so the runner moves to a leaf before turning the memory controller on below it.
    let leaf = dir.join("runner");
    create_dir(&leaf).map_err(|e| format!("{}: {e}", leaf.display()))?;
    std::fs::write(leaf.join("cgroup.procs"), "0")
        .map_err(|e| format!("moving the runner into {}: {e}", leaf.display()))?;
    std::fs::write(dir.join("cgroup.subtree_control"), "+memory")
        .map_err(|e| format!("enabling the memory controller in {}: {e}", dir.display()))?;
    Ok(Scope { dir, unit, cap })
}

/// Replaces this process with itself inside a new delegated, capped scope. Returns only on
/// failure.
fn reexec(cap: u64) -> String {
    let exe = match std::env::current_exe() {
        Ok(e) => e,
        Err(e) => return format!("cannot find the runner's own executable: {e}"),
    };
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let unit = format!("benchrun-{}-{nanos}", std::process::id());
    eprintln!("benchrun: re-running in scope {unit} with a memory cap of {} MiB", cap >> 20);
    let err = Command::new("systemd-run")
        .args(["--user", "--scope", "--quiet", "--collect"])
        .arg(format!("--unit={unit}"))
        .args(["-p", "Delegate=yes", "-p", "MemorySwapMax=0", "-p", "OOMPolicy=continue", "-p"])
        .arg(format!("MemoryMax={cap}"))
        .arg("--")
        .arg(exe)
        .args(std::env::args_os().skip(1))
        .env(MARKER, &unit)
        .exec();
    format!("benchrun run needs systemd-run --user to put it under a memory cap, and could not start it: {err}")
}

/// What a group's processes used, from the kernel's accounting.
#[derive(Clone, Copy, Debug, Default)]
pub struct Usage {
    pub cap: u64,
    /// `memory.peak`: everything charged to the group, which is its anonymous memory, its
    /// page tables and other kernel memory, and the file pages it was first to read. The
    /// binary and shared libraries, cached by an earlier run, are charged elsewhere, so this
    /// can be less than the peak resident set.
    pub peak: u64,
    /// Processes the kernel killed because the group reached its cap.
    pub oom_kills: u64,
    /// How often the group reached its cap and the kernel reclaimed memory from it.
    pub at_max: u64,
}

impl Usage {
    pub fn to_json(self) -> serde_json::Value {
        serde_json::json!({ "cap": self.cap, "peak": self.peak, "oom_kills": self.oom_kills, "at_max": self.at_max })
    }

    /// Why a run in this group cannot be used, if it cannot.
    pub fn problem(&self, who: &str) -> Option<String> {
        if self.oom_kills > 0 {
            Some(format!(
                "{who} was killed at its memory cap of {} MiB (cgroup peak {} MiB)",
                self.cap >> 20,
                self.peak >> 20
            ))
        } else if self.at_max > 0 {
            Some(format!(
                "{who} reached its memory cap of {} MiB, so the kernel reclaimed memory from it",
                self.cap >> 20
            ))
        } else {
            None
        }
    }
}

/// A cgroup for one process. It is removed when dropped.
pub struct Group {
    dir: PathBuf,
    cap: u64,
}

impl Scope {
    /// A fresh child cgroup with a hard limit of `cap` bytes, no swap, and the whole group
    /// killed together if it reaches the limit.
    pub fn group(&self, name: &str, cap: u64) -> io::Result<Group> {
        let dir = self.dir.join(name);
        create_dir(&dir)?;
        let g = Group { dir, cap };
        std::fs::write(g.dir.join("memory.max"), cap.to_string())?;
        std::fs::write(g.dir.join("memory.swap.max"), "0")?;
        std::fs::write(g.dir.join("memory.oom.group"), "1")?;
        Ok(g)
    }
}

impl Group {
    /// The file a process's ID is written to to move it into the group.
    pub fn procs(&self) -> PathBuf {
        self.dir.join("cgroup.procs")
    }

    /// The group's accounting. Read it after its processes have been reaped.
    pub fn usage(&self) -> Usage {
        let events = read(&self.dir.join("memory.events")).unwrap_or_default();
        let event = |k: &str| {
            events.lines().find_map(|l| l.strip_prefix(k)?.strip_prefix(' ')?.parse::<u64>().ok()).unwrap_or(0)
        };
        Usage {
            cap: self.cap,
            peak: read(&self.dir.join("memory.peak")).ok().and_then(|s| s.parse().ok()).unwrap_or(0),
            oom_kills: event("oom_kill"),
            at_max: event("max"),
        }
    }
}

impl Drop for Group {
    fn drop(&mut self) {
        // A group stays populated for a moment after its last process is reaped.
        for _ in 0..100 {
            if std::fs::remove_dir(&self.dir).is_ok() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
}

fn own_cgroup() -> io::Result<PathBuf> {
    let text = std::fs::read_to_string("/proc/self/cgroup")?;
    let rel = text.lines().find_map(|l| l.strip_prefix("0::")).ok_or_else(|| io::Error::other("no cgroup v2 entry"))?;
    Ok(Path::new("/sys/fs/cgroup").join(rel.trim_start_matches('/')))
}

fn create_dir(p: &Path) -> io::Result<()> {
    match std::fs::create_dir(p) {
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Ok(()),
        r => r,
    }
}

fn read(p: &Path) -> io::Result<String> {
    std::fs::read_to_string(p).map(|s| s.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes() {
        assert_eq!(parse_size("4G"), Some(4 << 30));
        assert_eq!(parse_size("512M"), Some(512 << 20));
        assert_eq!(parse_size("1000"), Some(1000));
        assert_eq!(parse_size("4g"), None);
        assert_eq!(parse_size(""), None);
    }

    #[test]
    fn scope_cap_is_bounded() {
        assert_eq!(scope_cap(DEFAULT_RUN), DEFAULT_RUN + DRIVER + RUNNER);
        assert_eq!(scope_cap(MAX), MAX);
    }
}
