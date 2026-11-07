//! Machine facts from /proc and /sys: CPU utilisation, SMT siblings, cache domains.
use std::collections::BTreeSet;

/// Busy and total jiffies per CPU, from /proc/stat.
#[derive(Clone, Debug, Default)]
pub struct CpuTimes(pub Vec<(u64, u64)>);

impl CpuTimes {
    pub fn read() -> CpuTimes {
        let text = std::fs::read_to_string("/proc/stat").unwrap_or_default();
        let mut v = Vec::new();
        for line in text.lines() {
            // Per-CPU lines only: the aggregate line is "cpu  <user> ...", with no number.
            let Some(rest) = line.strip_prefix("cpu").filter(|r| r.starts_with(|c: char| c.is_ascii_digit())) else {
                continue;
            };
            let mut it = rest.split_whitespace();
            let Some(id) = it.next().and_then(|s| s.parse::<usize>().ok()) else { continue };
            let f: Vec<u64> = it.filter_map(|s| s.parse().ok()).collect();
            // user nice system idle iowait irq softirq steal guest guest_nice; guest time is
            // already inside user.
            let total: u64 = f.iter().take(8).sum();
            let idle = f.get(3).copied().unwrap_or(0) + f.get(4).copied().unwrap_or(0);
            if v.len() <= id {
                v.resize(id + 1, (0, 0));
            }
            v[id] = (total - idle, total);
        }
        CpuTimes(v)
    }

    /// Busy fraction per CPU between `earlier` and `self`.
    pub fn busy_since(&self, earlier: &CpuTimes) -> Vec<f64> {
        self.0
            .iter()
            .enumerate()
            .map(|(i, &(b, t))| {
                let (b0, t0) = earlier.0.get(i).copied().unwrap_or((0, 0));
                if t > t0 { (b - b0) as f64 / (t - t0) as f64 } else { 0.0 }
            })
            .collect()
    }
}

/// Each CPU's time running tasks, in nanoseconds, by two clocks whose differences can be set
/// against a process's task clock to find other work on its CPUs. Neither is exact, each in
/// its own way, so the other-work check takes the smaller of the two (`serve::foreign`):
///
/// - `queue`: the run queue's CPU time in /proc/schedstat. Exact, but a task's time is added
///   only when it is switched out, so a worker that runs through a mark, as at saturation, is
///   counted in the next segment.
/// - `awake`: the monotonic clock less idle, iowait and steal time from /proc/stat. Current, but
///   on this kernel idle time is sampled at the tick, so a CPU that idles often, as a server's
///   does at partial load, reads up to half a CPU off either way.
///
/// Real other work shows in both. Each CPU's `ksoftirqd` is taken out of both: it runs the
/// network softirqs a busy CPU defers, the server's own traffic (a proxy defers a third of a
/// CPU at saturation), charged to no process.
#[derive(Clone, Debug, Default)]
pub struct TaskTime {
    pub queue: Vec<u64>,
    pub awake: Vec<u64>,
}

impl TaskTime {
    pub fn read() -> TaskTime {
        let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
        // SAFETY: clock_gettime writes one timespec.
        unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
        let now = ts.tv_sec as u64 * 1_000_000_000 + ts.tv_nsec as u64;
        // SAFETY: sysconf reads a constant.
        let hz = unsafe { libc::sysconf(libc::_SC_CLK_TCK) }.max(1) as u64;
        let mut awake = per_cpu_line("/proc/stat", |f| {
            // user nice system idle iowait irq softirq steal
            let away: u64 = [3, 4, 7].iter().map(|&i| f.get(i).copied().unwrap_or(0)).sum();
            now.saturating_sub(away * (1_000_000_000 / hz))
        });
        let mut queue = per_cpu_line("/proc/schedstat", |f| f.get(6).copied().unwrap_or(0));
        for (cpu, pid) in ksoftirqd().iter().enumerate() {
            let Some(pid) = pid else { continue };
            let s = std::fs::read_to_string(format!("/proc/{pid}/schedstat")).unwrap_or_default();
            let ran: u64 = s.split_whitespace().next().and_then(|x| x.parse().ok()).unwrap_or(0);
            for v in [&mut awake, &mut queue] {
                if let Some(t) = v.get_mut(cpu) {
                    *t = t.saturating_sub(ran);
                }
            }
        }
        TaskTime { queue, awake }
    }

    /// Other work over `cpus` between `earlier` and now, in CPUs, given the process's task
    /// time over the same `ns`: the smaller of the two clocks' estimates.
    pub fn foreign_since(&self, earlier: &TaskTime, cpus: &[usize], own_ns: u64, ns: u64) -> f64 {
        let est = |a: &[u64], b: &[u64]| {
            let on: u64 = cpus
                .iter()
                .map(|&c| a.get(c).copied().unwrap_or(0).saturating_sub(b.get(c).copied().unwrap_or(0)))
                .sum();
            (on as f64 - own_ns as f64) / ns.max(1) as f64
        };
        est(&self.queue, &earlier.queue).min(est(&self.awake, &earlier.awake))
    }
}

/// One number per `cpuN` line of a /proc file, indexed by N, from the line's fields after the
/// name.
fn per_cpu_line(path: &str, f: impl Fn(&[u64]) -> u64) -> Vec<u64> {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let mut v = Vec::new();
    for line in text.lines() {
        let Some(rest) = line.strip_prefix("cpu").filter(|r| r.starts_with(|c: char| c.is_ascii_digit())) else {
            continue;
        };
        let mut it = rest.split_whitespace();
        let Some(id) = it.next().and_then(|s| s.parse::<usize>().ok()) else { continue };
        let fields: Vec<u64> = it.filter_map(|s| s.parse().ok()).collect();
        if v.len() <= id {
            v.resize(id + 1, 0);
        }
        v[id] = f(&fields);
    }
    v
}

/// The process ID of each CPU's `ksoftirqd`, indexed by CPU, found once.
fn ksoftirqd() -> &'static [Option<u32>] {
    static PIDS: std::sync::OnceLock<Vec<Option<u32>>> = std::sync::OnceLock::new();
    PIDS.get_or_init(|| {
        let mut v = Vec::new();
        for e in std::fs::read_dir("/proc").into_iter().flatten().flatten() {
            let Some(pid) = e.file_name().to_str().and_then(|n| n.parse::<u32>().ok()) else { continue };
            let comm = std::fs::read_to_string(e.path().join("comm")).unwrap_or_default();
            let Some(cpu) = comm.trim().strip_prefix("ksoftirqd/").and_then(|c| c.parse::<usize>().ok()) else {
                continue;
            };
            if v.len() <= cpu {
                v.resize(cpu + 1, None);
            }
            v[cpu] = Some(pid);
        }
        v
    })
}

/// Sum of busy fractions over `cpus`: how many CPUs' worth of work ran there.
pub fn busy_sum(busy: &[f64], cpus: &BTreeSet<usize>) -> f64 {
    cpus.iter().map(|&c| busy.get(c).copied().unwrap_or(0.0)).sum()
}

pub fn online_cpus() -> Vec<usize> {
    let s = std::fs::read_to_string("/sys/devices/system/cpu/online").unwrap_or_default();
    bench_harness::Cpus::parse(s.trim()).map(|c| c.0).unwrap_or_default()
}

/// The SMT siblings of `cpu`, itself included.
pub fn siblings(cpu: usize) -> Vec<usize> {
    let p = format!("/sys/devices/system/cpu/cpu{cpu}/topology/thread_siblings_list");
    let s = std::fs::read_to_string(p).unwrap_or_default();
    bench_harness::Cpus::parse(s.trim()).map(|c| c.0).unwrap_or_else(|| vec![cpu])
}

/// The CPUs that share `cpu`'s last-level cache: its highest cache index, whatever the number.
pub fn cache_domain(cpu: usize) -> Vec<usize> {
    let dir = format!("/sys/devices/system/cpu/cpu{cpu}/cache");
    let top = std::fs::read_dir(&dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.file_name().to_str()?.strip_prefix("index")?.parse::<u32>().ok())
        .max();
    let s = top.and_then(|i| std::fs::read_to_string(format!("{dir}/index{i}/shared_cpu_list")).ok());
    s.and_then(|s| bench_harness::Cpus::parse(s.trim())).map(|c| c.0).unwrap_or_else(|| vec![cpu])
}

pub fn loadavg() -> [f64; 3] {
    let s = std::fs::read_to_string("/proc/loadavg").unwrap_or_default();
    let mut it = s.split_whitespace().filter_map(|x| x.parse().ok());
    [it.next().unwrap_or(0.0), it.next().unwrap_or(0.0), it.next().unwrap_or(0.0)]
}

/// This process's limit on address space (`RLIMIT_AS`), if it is finite. Every process the
/// runner starts inherits it.
pub fn address_space_limit() -> Option<u64> {
    let mut l = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
    // SAFETY: getrlimit writes one rlimit.
    let ok = unsafe { libc::getrlimit(libc::RLIMIT_AS, &mut l) } == 0;
    (ok && l.rlim_cur != libc::RLIM_INFINITY).then_some(l.rlim_cur)
}

pub fn read_trim(path: &str) -> String {
    std::fs::read_to_string(path).map(|s| s.trim().to_string()).unwrap_or_default()
}

/// The CPU's name, where /proc/cpuinfo has one. On aarch64 it does not, so the machine's name from
/// the device tree, if any, and the implementer and part numbers of the first core and the last.
pub fn cpu_model() -> String {
    let s = std::fs::read_to_string("/proc/cpuinfo").unwrap_or_default();
    let field = |name: &str| -> Vec<String> {
        s.lines()
            .filter(|l| l.starts_with(name))
            .filter_map(|l| l.split(':').nth(1).map(|v| v.trim().to_string()))
            .collect()
    };
    if let Some(model) = field("model name").into_iter().next() {
        return model;
    }
    let tree = std::fs::read_to_string("/proc/device-tree/model").unwrap_or_default();
    let (parts, implementer) = (field("CPU part"), field("CPU implementer"));
    let first = |v: &[String]| v.first().cloned().unwrap_or_default();
    let last = parts.last().cloned().unwrap_or_default();
    format!("{} (implementer {}, parts {} to {last})", tree.trim_end_matches('\0'), first(&implementer), first(&parts))
        .trim()
        .to_string()
}

/// The machine's transparent-huge-page and compaction counters (`/proc/vmstat`), whose change
/// over a run shows whether it got huge pages and what the kernel did to provide them. They
/// are machine-wide, so they include other processes' activity; on a quiet machine that is
/// little.
pub struct Vmstat(Vec<(String, u64)>);

impl Vmstat {
    pub fn read() -> Vmstat {
        let text = std::fs::read_to_string("/proc/vmstat").unwrap_or_default();
        Vmstat(
            text.lines()
                .filter(|l| l.starts_with("thp_") || l.starts_with("compact_"))
                .filter_map(|l| {
                    let (k, v) = l.split_once(' ')?;
                    Some((k.to_string(), v.trim().parse().ok()?))
                })
                .collect(),
        )
    }

    /// The counters that changed since this reading, by name.
    pub fn delta(&self) -> serde_json::Value {
        let mut m = serde_json::Map::new();
        for (k, v) in Vmstat::read().0 {
            let before = self.0.iter().find(|(b, _)| *b == k).map_or(0, |(_, x)| *x);
            if v != before {
                m.insert(k, v.saturating_sub(before).into());
            }
        }
        serde_json::Value::Object(m)
    }
}
