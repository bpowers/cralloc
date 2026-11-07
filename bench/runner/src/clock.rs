//! The clock a run starts with.
//!
//! On the reference machine (amd-pstate-epp, EPP `balance_performance`) the boost is a
//! property of the package: after a few hundred milliseconds with no core busy, every core
//! runs at the nominal clock (4.38 GHz) for the first 250 ms of work, wherever it runs, and
//! then boosts (5.56 to 5.71 GHz, by how many cores are busy). Any one busy core, in either
//! L3, keeps the boost for all of them, and it survives up to 200 ms of idle. So a short job
//! started after a pause runs its whole length at a clock 23% lower, taking 30% longer at
//! the same cycles (docs/research/hardware.md, "The clock a run starts with belongs to the
//! package"). Before each run the runner therefore spins on the job's own CPUs, measuring each
//! one's clock as cycles over wall time in short windows, until every one of them has held the
//! warm clock for a few windows in a row, and then starts the job. The spin is the check: a run
//! starts only once its CPUs were seen at the clock, not after a fixed time.
//!
//! The warm clock differs with how many cores are busy and with what else runs on the
//! machine, so it is learned rather than configured: the first warm-up of each CPU set spins
//! long enough to pass any ramp and takes the clock it settled at; later warm-ups wait for
//! that clock, and move it to whatever clock a long spin settles at if it has drifted.
use bench_harness::perf::{Counters, Event, Target};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// How the warm-up decides that the clock is there.
#[derive(Clone, Copy, Debug)]
pub struct Settings {
    /// The clock is measured over windows this long.
    pub window: Duration,
    /// Every CPU must be at the warm clock for this many windows in a row.
    pub hold: usize,
    /// Within this fraction of the warm clock counts as there.
    pub tolerance: f64,
    /// A first warm-up of a CPU set spins this long to learn its warm clock: well past the
    /// ramp from idle, which ends 300 ms into a spin on the reference machine.
    pub learn: Duration,
    /// A warm-up that has not reached the clock by now gives up, and the run records it.
    pub limit: Duration,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            window: Duration::from_millis(5),
            hold: 3,
            tolerance: 0.97,
            learn: Duration::from_millis(800),
            limit: Duration::from_millis(800),
        }
    }
}

/// What one warm-up did.
#[derive(Clone, Debug)]
pub struct Warmup {
    pub spun: Duration,
    /// The slowest CPU's clock over the last `hold` windows, GHz.
    pub ghz: f64,
    /// The warm clock it waited for, GHz.
    pub target: f64,
    pub reached: bool,
    /// This warm-up learned (or relearned) the CPU set's warm clock.
    pub learned: bool,
}

impl Warmup {
    pub fn to_json(&self) -> Value {
        json!({
            "spun_ms": self.spun.as_secs_f64() * 1e3,
            "ghz": self.ghz,
            "target_ghz": self.target,
            "reached": self.reached,
            "learned": self.learned,
        })
    }
}

/// The warm clock of each CPU set seen so far in a session.
pub struct Clock {
    settings: Settings,
    warm: BTreeMap<Vec<usize>, f64>,
}

impl Clock {
    pub fn new(settings: Settings) -> Clock {
        Clock { settings, warm: BTreeMap::new() }
    }

    /// Spins on `cpus` until each is at the set's warm clock, then returns at once, so the
    /// caller should start the job straight after.
    pub fn warm(&mut self, cpus: &[usize]) -> Warmup {
        let s = self.settings;
        let mut key = cpus.to_vec();
        key.sort_unstable();
        let known = self.warm.get(&key).copied();
        let spin = Spinners::start(cpus, s.window);
        let t0 = Instant::now();
        // The slowest CPU's clock in each completed window.
        let mut slowest: Vec<f64> = Vec::new();
        let held = |v: &[f64]| -> Option<f64> {
            (v.len() >= s.hold).then(|| {
                let tail = &v[v.len() - s.hold..];
                tail.iter().copied().fold(f64::INFINITY, f64::min)
            })
        };
        let result = loop {
            spin.wait_window(slowest.len() + 1);
            slowest.push(spin.slowest());
            let spun = t0.elapsed();
            let now = held(&slowest);
            match (known, now) {
                (Some(target), Some(g)) if g >= target * s.tolerance => {
                    break Warmup { spun, ghz: g, target, reached: true, learned: false };
                }
                (None, _) if spun >= s.learn => {
                    // The clock it settled at: the median of the last fifth of the spin,
                    // which is past any ramp.
                    let g = settled(&slowest);
                    self.warm.insert(key.clone(), g);
                    break Warmup { spun, ghz: g, target: g, reached: true, learned: true };
                }
                (Some(target), _) if spun >= s.limit => {
                    // The CPUs settled somewhere else: another load on the machine, or a
                    // warmer or cooler room. Relearn from this spin, which is as long as a
                    // learning one, and record that this run did not get the old clock.
                    let g = settled(&slowest);
                    self.warm.insert(key.clone(), g);
                    let ghz = now.unwrap_or(g);
                    break Warmup { spun, ghz, target, reached: ghz >= target * s.tolerance, learned: true };
                }
                _ => {}
            }
        };
        spin.stop();
        result
    }
}

/// Spins the calling thread for `d`, which keeps the package boosted.
pub fn spin(d: Duration) {
    let t = Instant::now();
    while t.elapsed() < d {
        std::hint::spin_loop();
    }
}

fn settled(windows: &[f64]) -> f64 {
    let tail = &windows[windows.len() - (windows.len() / 5).max(1)..];
    crate::stats::median(tail)
}

/// One spinning thread per CPU, each publishing its clock over its latest window.
struct Spinners {
    stop: Arc<AtomicBool>,
    /// Per thread: the clock of its latest window (f64 bits) and how many windows it has
    /// finished.
    slots: Arc<Vec<(AtomicU64, AtomicU64)>>,
    threads: Vec<std::thread::JoinHandle<()>>,
}

impl Spinners {
    fn start(cpus: &[usize], window: Duration) -> Spinners {
        let stop = Arc::new(AtomicBool::new(false));
        let slots: Arc<Vec<(AtomicU64, AtomicU64)>> =
            Arc::new(cpus.iter().map(|_| (AtomicU64::new(0), AtomicU64::new(0))).collect());
        let threads = cpus
            .iter()
            .enumerate()
            .map(|(i, &cpu)| {
                let (stop, slots) = (stop.clone(), slots.clone());
                std::thread::spawn(move || {
                    bench_harness::cpus::pin_to(&[cpu]);
                    // Without a counter the warm-up still spins, and reports 0 GHz, which
                    // the run records as not reached.
                    let counter = Counters::open(&[Event::Cycles], Target::Me).ok();
                    let cycles = || counter.as_ref().and_then(|c| c.read().get("cycles")).unwrap_or(0);
                    if let Some(c) = &counter {
                        c.enable();
                    }
                    let (mut t, mut c) = (Instant::now(), cycles());
                    while !stop.load(Ordering::Relaxed) {
                        let e = t.elapsed();
                        if e < window {
                            std::hint::spin_loop();
                            continue;
                        }
                        let (now, cn) = (Instant::now(), cycles());
                        let ghz = (cn - c) as f64 / now.duration_since(t).as_nanos() as f64;
                        slots[i].0.store(ghz.to_bits(), Ordering::Relaxed);
                        slots[i].1.fetch_add(1, Ordering::Release);
                        (t, c) = (now, cn);
                    }
                })
            })
            .collect();
        Spinners { stop, slots, threads }
    }

    /// Waits until every thread has finished `n` windows.
    fn wait_window(&self, n: usize) {
        while self.slots.iter().any(|s| (s.1.load(Ordering::Acquire) as usize) < n) {
            std::thread::sleep(Duration::from_micros(500));
        }
    }

    fn slowest(&self) -> f64 {
        self.slots.iter().map(|s| f64::from_bits(s.0.load(Ordering::Relaxed))).fold(f64::INFINITY, f64::min)
    }

    fn stop(self) {
        self.stop.store(true, Ordering::Relaxed);
        for t in self.threads {
            let _ = t.join();
        }
    }
}

/// The clock a run got over what it measured: cycles over on-CPU time, from the workload's
/// own phase counters when it has them, else the runner's window for a server, else the whole
/// process. A wait for another thread costs no cycles and no on-CPU time, so this is the
/// clock its threads ran at while they ran.
pub fn of_run(workload: Option<&Value>, window: Option<&Value>, process: &Value) -> Option<(f64, &'static str)> {
    let ratio = |v: &Value| {
        let (c, t) = (v["cycles"].as_f64()?, v["task_clock_ns"].as_f64()?);
        (t > 0.0).then(|| c / t)
    };
    if let Some(phases) = workload.and_then(|w| w["phases"].as_object()).filter(|p| !p.is_empty()) {
        let (mut c, mut t) = (0.0, 0.0);
        for p in phases.values() {
            if let (Some(pc), Some(pt)) = (p["cycles"].as_f64(), p["task_clock_ns"].as_f64()) {
                c += pc;
                t += pt;
            }
        }
        if t > 0.0 {
            return Some((c / t, "phases"));
        }
    }
    if let Some(g) = window.and_then(ratio) {
        return Some((g, "window"));
    }
    ratio(process).map(|g| (g, "process"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_clock_prefers_the_measured_phases() {
        let process = json!({ "cycles": 1000, "task_clock_ns": 1000 });
        let workload = json!({ "phases": {
            "a": { "cycles": 5000, "task_clock_ns": 1000 },
            "b": { "cycles": 3000, "task_clock_ns": 1000 },
        }});
        assert_eq!(of_run(Some(&workload), None, &process), Some((4.0, "phases")));
        let window = json!({ "cycles": 2000, "task_clock_ns": 1000 });
        assert_eq!(of_run(Some(&json!({ "phases": {} })), Some(&window), &process), Some((2.0, "window")));
        assert_eq!(of_run(None, Some(&Value::Null), &process), Some((1.0, "process")));
        assert_eq!(of_run(None, None, &json!({ "cycles": 5, "task_clock_ns": 0 })), None);
    }

    #[test]
    fn settled_is_the_median_of_the_last_fifth() {
        let mut w = vec![4.4; 40];
        w.extend([5.7, 5.6, 5.7, 5.8, 5.7, 5.7, 5.7, 5.7, 5.7, 5.7]);
        assert_eq!(settled(&w), 5.7);
        assert_eq!(settled(&[4.0]), 4.0);
    }
}
