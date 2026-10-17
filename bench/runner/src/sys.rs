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
            let Some(rest) = line.strip_prefix("cpu") else { continue };
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

/// The CPUs that share `cpu`'s last-level cache.
pub fn l3_domain(cpu: usize) -> Vec<usize> {
    let p = format!("/sys/devices/system/cpu/cpu{cpu}/cache/index3/shared_cpu_list");
    let s = std::fs::read_to_string(p).unwrap_or_default();
    bench_harness::Cpus::parse(s.trim()).map(|c| c.0).unwrap_or_else(|| vec![cpu])
}

pub fn loadavg() -> [f64; 3] {
    let s = std::fs::read_to_string("/proc/loadavg").unwrap_or_default();
    let mut it = s.split_whitespace().filter_map(|x| x.parse().ok());
    [it.next().unwrap_or(0.0), it.next().unwrap_or(0.0), it.next().unwrap_or(0.0)]
}

pub fn read_trim(path: &str) -> String {
    std::fs::read_to_string(path).map(|s| s.trim().to_string()).unwrap_or_default()
}

pub fn cpu_model() -> String {
    let s = std::fs::read_to_string("/proc/cpuinfo").unwrap_or_default();
    s.lines()
        .find_map(|l| l.strip_prefix("model name").and_then(|r| r.split(':').nth(1)).map(|m| m.trim().to_string()))
        .unwrap_or_default()
}
