//! Starting a measured process: pinned before `execve`, counted from its first instruction,
//! its stdout read line by line, its memory sampled, and its resource usage collected when
//! it is reaped.
//!
//! The child is forked by hand rather than with `std::process::Command` so that it can wait
//! between `fork` and `execve` while the parent attaches counters to it: counters opened with
//! `enable_on_exec` on a process that has not yet exec'd count the whole program and, with
//! `inherit`, every thread it creates. This is how `perf stat` does it.
use bench_harness::mem::Rollup;
use bench_harness::perf::{Counters, Sample, Target, process_events};
use std::cell::Cell;
use std::ffi::CString;
use std::fs::File;
use std::io::{self, BufRead, BufReader};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

pub struct Spec<'a> {
    pub argv: &'a [String],
    /// The complete environment of the child.
    pub env: &'a [(String, String)],
    /// The child's CPU affinity, inherited by all its threads.
    pub cpus: &'a [usize],
    pub stderr: &'a Path,
    /// Attach process-wide counters.
    pub count: bool,
    /// The `cgroup.procs` file of the cgroup the child must run in. It is moved there before
    /// it execs, so everything the program does is charged to that cgroup and capped by it.
    pub cgroup: &'a Path,
}

pub struct Child {
    pub pid: i32,
    pidfd: OwnedFd,
    pub counters: Option<Counters>,
    pub started: Instant,
    /// When `wait_exit` first saw the process gone: the end of its wall time.
    exited: Cell<Option<Instant>>,
    /// Lines of stdout with the time they arrived. Disconnects at EOF.
    pub lines: Receiver<(Instant, String)>,
}

fn cstrings(v: impl Iterator<Item = String>) -> io::Result<Vec<CString>> {
    v.map(|s| CString::new(s).map_err(|_| io::Error::other("NUL in argument"))).collect()
}

fn pipe() -> io::Result<(OwnedFd, OwnedFd)> {
    let mut fds = [0i32; 2];
    // SAFETY: fds has room for two descriptors.
    if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) } != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: both were just created and are ours.
    Ok(unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) })
}

impl Child {
    pub fn spawn(spec: &Spec) -> io::Result<Child> {
        let argv = cstrings(spec.argv.iter().cloned())?;
        let envp = cstrings(spec.env.iter().map(|(k, v)| format!("{k}={v}")))?;
        let mut argv_p: Vec<*const libc::c_char> = argv.iter().map(|s| s.as_ptr()).collect();
        argv_p.push(std::ptr::null());
        let mut envp_p: Vec<*const libc::c_char> = envp.iter().map(|s| s.as_ptr()).collect();
        envp_p.push(std::ptr::null());
        // SAFETY: cpu_set_t is plain data; CPU_SET bounds-checks.
        let mut set: libc::cpu_set_t = unsafe { std::mem::zeroed() };
        for &c in spec.cpus {
            unsafe { libc::CPU_SET(c, &mut set) };
        }
        let (out_r, out_w) = pipe()?;
        let (sync_r, sync_w) = pipe()?;
        let (err_r, err_w) = pipe()?;
        let stderr = File::create(spec.stderr)?;

        // SAFETY: between fork and execve the child calls only async-signal-safe functions
        // on memory prepared before the fork, then execs or exits.
        let pid = unsafe { libc::fork() };
        if pid < 0 {
            return Err(io::Error::last_os_error());
        }
        if pid == 0 {
            unsafe {
                libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL);
                // Rust ignores SIGPIPE in its own processes; give the child the default back.
                libc::signal(libc::SIGPIPE, libc::SIG_DFL);
                libc::sched_setaffinity(0, std::mem::size_of::<libc::cpu_set_t>(), &set);
                libc::dup2(out_w.as_raw_fd(), 1);
                libc::dup2(stderr.as_raw_fd(), 2);
                let mut b = 0u8;
                libc::read(sync_r.as_raw_fd(), (&mut b as *mut u8).cast(), 1);
                libc::execve(argv_p[0], argv_p.as_ptr(), envp_p.as_ptr());
                let e: i32 = *libc::__errno_location();
                libc::write(err_w.as_raw_fd(), (&e as *const i32).cast(), 4);
                libc::_exit(127);
            }
        }
        drop((out_w, sync_r, err_w, stderr));
        let kill = || {
            // SAFETY: kill and reap our own child, which is still waiting to exec.
            unsafe {
                libc::kill(pid, libc::SIGKILL);
                libc::waitpid(pid, std::ptr::null_mut(), 0);
            }
        };
        if let Err(e) = std::fs::write(spec.cgroup, pid.to_string()) {
            kill();
            return Err(io::Error::other(format!("moving the child into {}: {e}", spec.cgroup.display())));
        }
        let counters = if spec.count {
            match Counters::open_on(&process_events(), Target::ChildBeforeExec(pid), spec.cpus) {
                Ok(c) => Some(c),
                Err(e) => {
                    kill();
                    return Err(io::Error::other(format!("perf_event_open on child: {e}")));
                }
            }
        } else {
            None
        };
        let started = Instant::now();
        // SAFETY: a one-byte write to a pipe we own; then closing it releases the child.
        unsafe { libc::write(sync_w.as_raw_fd(), [1u8].as_ptr().cast(), 1) };
        drop(sync_w);
        let mut e = [0u8; 4];
        // SAFETY: reading at most 4 bytes into e.
        let n = unsafe { libc::read(err_r.as_raw_fd(), e.as_mut_ptr().cast(), 4) };
        if n == 4 {
            unsafe { libc::waitpid(pid, std::ptr::null_mut(), 0) };
            return Err(io::Error::from_raw_os_error(i32::from_ne_bytes(e)));
        }
        // SAFETY: pidfd_open on our own, unreaped child.
        let pidfd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) };
        if pidfd < 0 {
            let e = io::Error::last_os_error();
            // SAFETY: kill and reap our own child, which has already exec'd.
            unsafe {
                libc::kill(pid, libc::SIGKILL);
                libc::waitpid(pid, std::ptr::null_mut(), 0);
            }
            return Err(e);
        }
        let pidfd = unsafe { OwnedFd::from_raw_fd(pidfd as i32) };
        let (tx, rx) = channel();
        let out = File::from(out_r);
        std::thread::spawn(move || {
            for line in BufReader::new(out).lines() {
                let Ok(line) = line else { break };
                if tx.send((Instant::now(), line)).is_err() {
                    break;
                }
            }
        });
        Ok(Child { pid, pidfd, counters, started, exited: Cell::new(None), lines: rx })
    }

    /// Waits until the process has exited (it stays unreaped), or `timeout` passes.
    pub fn wait_exit(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let mut p = libc::pollfd { fd: self.pidfd.as_raw_fd(), events: libc::POLLIN, revents: 0 };
            // SAFETY: one valid pollfd.
            let r = unsafe { libc::poll(&mut p, 1, left.as_millis().min(i32::MAX as u128) as i32) };
            if r > 0 {
                if self.exited.get().is_none() {
                    self.exited.set(Some(Instant::now()));
                }
                return true;
            }
            if r == 0 || left.is_zero() {
                return false;
            }
        }
    }

    pub fn signal(&self, sig: i32) {
        // SAFETY: pidfd_send_signal to our own child; harmless if it has exited.
        unsafe { libc::syscall(libc::SYS_pidfd_send_signal, self.pidfd.as_raw_fd(), sig, std::ptr::null::<u8>(), 0) };
    }

    /// The counters now (they keep their final values after the process exits).
    pub fn read_counters(&self) -> Sample {
        self.counters.as_ref().map(|c| c.read()).unwrap_or_default()
    }

    /// Reaps the process. Call after `wait_exit` returned true, or after killing it.
    pub fn reap(&self) -> Exit {
        let mut status = 0i32;
        // SAFETY: rusage is plain data; wait4 on our own child.
        let mut ru: libc::rusage = unsafe { std::mem::zeroed() };
        unsafe { libc::wait4(self.pid, &mut status, 0, &mut ru) };
        let end = self.exited.get().unwrap_or_else(Instant::now);
        let wall_ns = end.duration_since(self.started).as_nanos() as u64;
        let tv = |t: libc::timeval| t.tv_sec as u64 * 1_000_000_000 + t.tv_usec as u64 * 1000;
        Exit {
            code: libc::WIFEXITED(status).then(|| libc::WEXITSTATUS(status)),
            signal: libc::WIFSIGNALED(status).then(|| libc::WTERMSIG(status)),
            wall_ns,
            utime_ns: tv(ru.ru_utime),
            stime_ns: tv(ru.ru_stime),
            maxrss: ru.ru_maxrss as u64 * 1024,
            minflt: ru.ru_minflt as u64,
            majflt: ru.ru_majflt as u64,
            nvcsw: ru.ru_nvcsw as u64,
            nivcsw: ru.ru_nivcsw as u64,
        }
    }
}

/// How a process ended, and what it used, from `wait4`.
#[derive(Clone, Debug, Default)]
pub struct Exit {
    pub code: Option<i32>,
    pub signal: Option<i32>,
    /// From the moment the child was released to exec until it was seen to exit.
    pub wall_ns: u64,
    pub utime_ns: u64,
    pub stime_ns: u64,
    /// `ru_maxrss`. Not the child's peak: after fork and exec it is at least the runner's
    /// own resident set at the fork. Kept for completeness; use `VmHWM` instead.
    pub maxrss: u64,
    pub minflt: u64,
    pub majflt: u64,
    pub nvcsw: u64,
    pub nivcsw: u64,
}

impl Exit {
    pub fn ok(&self) -> bool {
        self.code == Some(0)
    }
}

/// One memory sample: the resident set and its peak so far, from `statm` and `status`,
/// which are cheap, and every so often the full `smaps_rollup`.
#[derive(Clone, Copy, Debug)]
pub struct MemSample {
    pub t_ms: f64,
    pub rss: u64,
    pub hwm: u64,
    pub rollup: Option<Rollup>,
    /// How many processes the sample summed.
    pub procs: usize,
}

/// Samples a process's memory on a thread of its own until stopped, over every process of its
/// cgroup, so that a server that forks workers (Sōzu) is counted whole. With one process, its
/// resident set and peak. With several, which share the executable's pages, each process's
/// resident set less its file-backed and shared pages, plus the largest of those once: the
/// anonymous memory of all of them and the executable counted once; the peak is then the
/// largest such sum sampled. Rollups are summed, so their proportional set is exact.
///
/// `smaps_rollup` walks the page tables under the process's `mmap_lock`, which for a
/// large heap takes milliseconds and delays the process's own `munmap` and `mprotect`. So
/// it is read only every `rollup_every`; the frequent samples read counters only.
pub struct Sampler {
    stop: Arc<AtomicBool>,
    handle: std::thread::JoinHandle<Vec<MemSample>>,
}

impl Sampler {
    pub fn start(pid: i32, procs: PathBuf, since: Instant, interval: Duration, rollup_every: Duration) -> Sampler {
        let stop = Arc::new(AtomicBool::new(false));
        let s = stop.clone();
        let handle = std::thread::spawn(move || {
            let mut v = Vec::new();
            let mut next_rollup = Instant::now();
            while !s.load(Ordering::Acquire) {
                let mut pids: Vec<i32> = std::fs::read_to_string(&procs)
                    .unwrap_or_default()
                    .lines()
                    .filter_map(|l| l.trim().parse().ok())
                    .collect();
                if pids.is_empty() {
                    pids.push(pid);
                }
                let (rss, hwm) = if pids.len() == 1 {
                    let rss = bench_harness::mem::rss_of(pids[0]).unwrap_or(0);
                    (rss, bench_harness::mem::vm_hwm(pids[0]).unwrap_or(0))
                } else {
                    let parts: Vec<(u64, u64)> = pids.iter().filter_map(|&p| statm(p)).collect();
                    let shared = parts.iter().map(|p| p.1).max().unwrap_or(0);
                    let rss = parts.iter().map(|p| p.0.saturating_sub(p.1)).sum::<u64>() + shared;
                    (rss, v.iter().map(|s: &MemSample| s.rss).max().unwrap_or(0).max(rss))
                };
                if rss > 0 {
                    let rollup = if Instant::now() >= next_rollup { Rollup::sum(&pids) } else { None };
                    if rollup.is_some() {
                        next_rollup = Instant::now() + rollup_every;
                    }
                    let procs = pids.len();
                    v.push(MemSample { t_ms: since.elapsed().as_secs_f64() * 1e3, rss, hwm, rollup, procs });
                }
                std::thread::park_timeout(interval);
            }
            v
        });
        Sampler { stop, handle }
    }

    pub fn stop(self) -> Vec<MemSample> {
        self.stop.store(true, Ordering::Release);
        self.handle.thread().unpark();
        self.handle.join().unwrap_or_default()
    }
}

/// A process's resident and shared (file-backed and shmem) bytes, from `statm`.
fn statm(pid: i32) -> Option<(u64, u64)> {
    let s = std::fs::read_to_string(format!("/proc/{pid}/statm")).ok()?;
    let mut f = s.split_whitespace().skip(1).map(|x| x.parse::<u64>().ok());
    let page = bench_harness::mem::page_size();
    Some((f.next()?? * page, f.next()?? * page))
}

/// CPU time and faults of a live process, from /proc/<pid>/stat.
#[derive(Clone, Copy, Debug, Default)]
pub struct ProcStat {
    pub utime_ticks: u64,
    pub stime_ticks: u64,
    pub minflt: u64,
}

impl ProcStat {
    pub fn read(pid: i32) -> ProcStat {
        let s = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap_or_default();
        // The command name is in parentheses and may contain spaces; fields follow the last ')'.
        let rest = s.rsplit_once(')').map(|(_, r)| r).unwrap_or("");
        let f: Vec<u64> = rest.split_whitespace().map(|x| x.parse().unwrap_or(0)).collect();
        // After the name: state(0) ppid(1) ... minflt(7) cminflt(8) majflt(9) cmajflt(10)
        // utime(11) stime(12).
        let g = |i: usize| f.get(i).copied().unwrap_or(0);
        ProcStat { utime_ticks: g(11), stime_ticks: g(12), minflt: g(7) }
    }
}
