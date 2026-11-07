//! The idle-state counters of a server's CPUs, read at each of the load generator's marks, so
//! that every group of segments says how often those CPUs entered a deep idle state.
//!
//! Why: a request that finds the CPU that would run it in a deep state waits for the exit
//! (18 us for C2 on the reference host), and at one fixed rate the faster server idles longer
//! between requests and goes deep more often. So a tail at a common rate can rank the idle
//! states and not the allocator (docs/research/measuring.md, "A tail at one fixed rate can rank
//! the CPU's idle states"). The counters say how much deep idle took part in each window, and
//! the report marks a comparison of tails whose deep-idle rates differ (`confounded`).
//!
//! The kernel keeps, per CPU and state, under `<root>/cpuN/cpuidle/stateK/`: `name`, the
//! declared exit `latency` in microseconds, `usage` (entries), `time` (microseconds in the
//! state, added when the CPU leaves it) and `disable`. Reading them needs no privileges; a
//! host or container without them records why.
use serde_json::{Value, json};
use std::fs::File;
use std::os::unix::fs::FileExt;
use std::path::Path;

/// Where the kernel puts the CPUs.
pub const SYSFS_CPU: &str = "/sys/devices/system/cpu";

/// A state is deep when the kernel declares an exit latency above this, in microseconds.
/// On the reference host POLL (0) and C1 (1) cost less than the wake every request pays
/// anyway (4 to 8 us from when it was due to the first server thread on a CPU), while C2 (18)
/// and C3 (350) are the exits that set the Connect h2 tail. Any cut-off from 1 to 17 sorts
/// this host's states the same way; 5 keeps a state of a couple of microseconds, as other
/// hosts declare for C1E, with the shallow ones. `scripts/deep-idle` reads this line.
pub const DEEP_LATENCY_US: u64 = 5;

/// Two allocators' deep-idle rates (entries per thousand requests) differ materially, so that
/// a difference between their tails may be idle exits and not the allocator, when the higher
/// is at least this many times the lower...
///
/// Chosen from the 26 valid runs of a session of 2026-10-24, each run's
/// deep entries over its `s70` window against its p99.9 and p99. Runs of one mode at one rate
/// differ by at most 1.09 when something other than idle sets the tail (`orders axum h2` at
/// 166k req/s: every run goes deep 84 to 91 times a thousand requests, and p99.9 still differ
/// by up to 20 us). On `connect log h2` at 63k, a run in the idle-exit mode (p99.9 more than
/// 5 us above its p99) against one out of it differs by 1.23 or more in 33 of 36 pairs. Two
/// runs that are both out of the mode differ by up to 1.57, and 9 of their 17 pairs reach 1.2,
/// so the mark errs towards saying a difference may be idle: it means "cannot tell", not
/// "is idle".
pub const CONFOUND_RATIO: f64 = 1.2;
/// ...and higher by at least this many entries per thousand requests. Near zero a ratio means
/// nothing: a server whose CPUs go deep once in ten thousand requests should not be flagged
/// against one that never does, since too few of its requests can meet an exit to set a
/// p99.9.
pub const CONFOUND_FLOOR: f64 = 1.0;

/// Whether a difference in tails between two windows whose CPUs entered deep idle states `a`
/// and `b` times per thousand requests may be the idle states rather than the allocators.
pub fn confounded(a: f64, b: f64) -> bool {
    let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
    hi - lo >= CONFOUND_FLOOR && hi >= lo * CONFOUND_RATIO
}

/// One state of one CPU, its description read once and its counters kept open.
struct StateFiles {
    name: String,
    latency_us: u64,
    usage: File,
    time: File,
    disable: File,
}

/// The counters of a set of CPUs, opened once so that each reading is a few reads.
pub struct Reader {
    cpus: Vec<(usize, Vec<StateFiles>)>,
}

/// One state of one CPU at one instant.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StateCount {
    pub name: String,
    pub latency_us: u64,
    pub usage: u64,
    pub time_us: u64,
    pub disabled: bool,
}

/// Every CPU's states at one instant, in the order the CPUs were given.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reading {
    pub cpus: Vec<(usize, Vec<StateCount>)>,
}

fn read_trim(p: &Path) -> Result<String, String> {
    std::fs::read_to_string(p).map(|s| s.trim().to_string()).map_err(|e| format!("{}: {e}", p.display()))
}

fn parse_u64(s: &str, what: &dyn std::fmt::Display) -> Result<u64, String> {
    s.trim().parse().map_err(|_| format!("{what}: not a number: {:?}", s.trim()))
}

/// A counter file's number, read from its start: a sysfs attribute is produced afresh by
/// every read at offset 0.
fn read_counter(f: &File, what: &dyn std::fmt::Display) -> Result<u64, String> {
    let mut buf = [0u8; 32];
    let n = f.read_at(&mut buf, 0).map_err(|e| format!("{what}: {e}"))?;
    parse_u64(&String::from_utf8_lossy(&buf[..n]), what)
}

impl Reader {
    /// Opens every idle state's counters of `cpus` under `root` (`SYSFS_CPU`, or a fixture
    /// shaped like it). Fails, saying why, when a CPU has none.
    pub fn open(root: &Path, cpus: &[usize]) -> Result<Reader, String> {
        let mut out = Vec::new();
        for &c in cpus {
            let dir = root.join(format!("cpu{c}")).join("cpuidle");
            let mut states: Vec<(u32, std::path::PathBuf)> = std::fs::read_dir(&dir)
                .map_err(|e| format!("no idle-state counters for CPU {c} ({}: {e})", dir.display()))?
                .filter_map(|e| e.ok())
                .filter_map(|e| {
                    let k = e.file_name().to_str()?.strip_prefix("state")?.parse().ok()?;
                    Some((k, e.path()))
                })
                .collect();
            if states.is_empty() {
                return Err(format!("no idle states for CPU {c} in {}", dir.display()));
            }
            states.sort();
            let mut files = Vec::new();
            for (_, s) in states {
                let open = |f: &str| File::open(s.join(f)).map_err(|e| format!("{}: {e}", s.join(f).display()));
                let latency = s.join("latency");
                files.push(StateFiles {
                    name: read_trim(&s.join("name"))?,
                    latency_us: parse_u64(&read_trim(&latency)?, &latency.display())?,
                    usage: open("usage")?,
                    time: open("time")?,
                    disable: open("disable")?,
                });
            }
            out.push((c, files));
        }
        Ok(Reader { cpus: out })
    }

    /// Every state's counters now. Fails when one cannot be read, as when its CPU has gone
    /// offline since `open`.
    pub fn read(&self) -> Result<Reading, String> {
        let mut cpus = Vec::with_capacity(self.cpus.len());
        for (c, files) in &self.cpus {
            let mut states = Vec::with_capacity(files.len());
            for f in files {
                let what = |k: &str| format!("CPU {c} {} {k}", f.name);
                states.push(StateCount {
                    name: f.name.clone(),
                    latency_us: f.latency_us,
                    usage: read_counter(&f.usage, &what("usage"))?,
                    time_us: read_counter(&f.time, &what("time"))?,
                    disabled: read_counter(&f.disable, &what("disable"))? != 0,
                });
            }
            cpus.push((*c, states));
        }
        Ok(Reading { cpus })
    }
}

/// One state over a window, summed over the CPUs: a state is the same one on every CPU when
/// it has the same name and exit latency.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StateTotal {
    pub name: String,
    pub latency_us: u64,
    pub entries: u64,
    pub time_us: u64,
    /// The CPUs on which it was disabled throughout.
    pub disabled_on: Vec<usize>,
}

impl StateTotal {
    pub fn deep(&self) -> bool {
        self.latency_us > DEEP_LATENCY_US
    }
}

/// What the CPUs' idle states did between two readings, or over several such spans.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Window {
    pub cpus: Vec<usize>,
    pub states: Vec<StateTotal>,
}

/// Adds one CPU's state to the totals, keyed by name and latency, in order of first sight.
fn add_state(states: &mut Vec<StateTotal>, cpu: usize, name: &str, latency_us: u64, d: (u64, u64), disabled: bool) {
    let i = match states.iter().position(|s| s.name == name && s.latency_us == latency_us) {
        Some(i) => i,
        None => {
            states.push(StateTotal { name: name.to_string(), latency_us, entries: 0, time_us: 0, disabled_on: vec![] });
            states.len() - 1
        }
    };
    let s = &mut states[i];
    s.entries += d.0;
    s.time_us += d.1;
    if disabled {
        s.disabled_on.push(cpu);
    }
}

impl Window {
    /// The counts from `a` to `b`. Fails, and the window is then not recorded, when the CPUs
    /// or their states differ between the readings, a state was enabled or disabled in
    /// between, or a counter went backwards: a CPU taken offline and back starts its counters
    /// again from zero, and a counter that wrapped reads the same way. Neither leaves a count
    /// that can be trusted, so none is made up.
    pub fn between(a: &Reading, b: &Reading) -> Result<Window, String> {
        let cpus: Vec<usize> = a.cpus.iter().map(|(c, _)| *c).collect();
        if cpus != b.cpus.iter().map(|(c, _)| *c).collect::<Vec<_>>() {
            return Err("the readings are of different CPUs".into());
        }
        let mut states = Vec::new();
        for ((c, sa), (_, sb)) in a.cpus.iter().zip(&b.cpus) {
            if sa.len() != sb.len() || sa.iter().zip(sb).any(|(x, y)| x.name != y.name || x.latency_us != y.latency_us)
            {
                return Err(format!("CPU {c}'s idle states changed during the window"));
            }
            for (x, y) in sa.iter().zip(sb) {
                if x.disabled != y.disabled {
                    return Err(format!("CPU {c} {} was enabled or disabled during the window", x.name));
                }
                if y.usage < x.usage || y.time_us < x.time_us {
                    return Err(format!(
                        "CPU {c} {}'s counters went backwards (usage {} to {}, time {} to {} us): the CPU went \
                         offline and back, or a counter wrapped",
                        x.name, x.usage, y.usage, x.time_us, y.time_us
                    ));
                }
                add_state(
                    &mut states,
                    *c,
                    &x.name,
                    x.latency_us,
                    (y.usage - x.usage, y.time_us - x.time_us),
                    x.disabled,
                );
            }
        }
        Ok(Window { cpus, states })
    }

    /// Adds another span of the same CPUs and states, as for a group of several segments.
    pub fn add(&mut self, other: &Window) -> Result<(), String> {
        let same = self.cpus == other.cpus
            && self.states.len() == other.states.len()
            && self
                .states
                .iter()
                .zip(&other.states)
                .all(|(x, y)| x.name == y.name && x.latency_us == y.latency_us && x.disabled_on == y.disabled_on);
        if !same {
            return Err("the idle states or their setting differ between the group's segments".into());
        }
        for (x, y) in self.states.iter_mut().zip(&other.states) {
            x.entries += y.entries;
            x.time_us += y.time_us;
        }
        Ok(())
    }

    pub fn deep_entries(&self) -> u64 {
        self.states.iter().filter(|s| s.deep()).map(|s| s.entries).sum()
    }

    /// Deep entries per thousand of the window's `requests`; none without requests.
    pub fn deep_per_kreq(&self, requests: f64) -> Option<f64> {
        (requests > 0.0).then(|| self.deep_entries() as f64 * 1e3 / requests)
    }

    /// The record of the window: per state its entries, time, name, exit latency and the CPUs
    /// where it was disabled, and the deep entries in all and per thousand requests.
    pub fn to_json(&self, requests: f64) -> Value {
        json!({
            "recorded": true,
            "cpus": bench_harness::Cpus(self.cpus.clone()).to_list(),
            "deep_latency_us": DEEP_LATENCY_US,
            "deep_entries": self.deep_entries(),
            "deep_per_kreq": self.deep_per_kreq(requests),
            "states": self.states.iter().map(|s| json!({
                "name": s.name,
                "latency_us": s.latency_us,
                "entries": s.entries,
                "time_us": s.time_us,
                "disabled_on": s.disabled_on,
            })).collect::<Vec<_>>(),
        })
    }
}

/// The record of a window that could not be counted.
pub fn not_recorded(why: &str) -> Value {
    json!({ "recorded": false, "why": why })
}

/// The deep entries per thousand requests of a window's record (`Window::to_json`); none when
/// the record is missing, as in results from before the runner kept it, or says it could not
/// be counted.
pub fn per_kreq(idle: &Value) -> Option<f64> {
    if idle["recorded"] != true {
        return None;
    }
    idle["deep_per_kreq"].as_f64()
}

/// The deep states of `cpus` and where they are enabled, as at the start of a run: the setting
/// the run measured under, for its record, and for the session's warning.
pub struct Setting {
    pub cpus: Vec<usize>,
    /// Each state's totals of one reading: only the names, latencies and `disabled_on` mean
    /// anything here.
    pub states: Vec<StateTotal>,
}

impl Setting {
    pub fn of(r: &Reading) -> Setting {
        let mut states = Vec::new();
        for (c, ss) in &r.cpus {
            for s in ss {
                add_state(&mut states, *c, &s.name, s.latency_us, (0, 0), s.disabled);
            }
        }
        Setting { cpus: r.cpus.iter().map(|(c, _)| *c).collect(), states }
    }

    /// The CPUs on which some deep state is enabled.
    pub fn deep_enabled_on(&self) -> Vec<usize> {
        self.cpus
            .iter()
            .copied()
            .filter(|c| self.states.iter().any(|s| s.deep() && !s.disabled_on.contains(c)))
            .collect()
    }

    pub fn deep_names(&self) -> Vec<&str> {
        self.states.iter().filter(|s| s.deep()).map(|s| s.name.as_str()).collect()
    }

    pub fn to_json(&self) -> Value {
        json!({
            "cpus": bench_harness::Cpus(self.cpus.clone()).to_list(),
            "deep_latency_us": DEEP_LATENCY_US,
            "deep_enabled_on": self.deep_enabled_on(),
            "states": self.states.iter().map(|s| json!({
                "name": s.name, "latency_us": s.latency_us, "deep": s.deep(), "disabled_on": s.disabled_on,
            })).collect::<Vec<_>>(),
        })
    }
}

/// The setting of `cpus` now, for a record: `Setting::to_json`, or why there is none.
pub fn setting_json(root: &Path, cpus: &[usize]) -> Value {
    match Reader::open(root, cpus).and_then(|r| r.read()) {
        Ok(r) => Setting::of(&r).to_json(),
        Err(e) => json!({ "cpus": bench_harness::Cpus(cpus.to_vec()).to_list(), "absent": e }),
    }
}

/// The line a server session prints when it starts with deep states enabled on its servers'
/// CPUs, or without the counters to tell; none when every deep state there is disabled.
pub fn warning(root: &Path, cpus: &[usize]) -> Option<String> {
    let list = |c: &[usize]| bench_harness::Cpus(c.to_vec()).to_list();
    let s = match Reader::open(root, cpus).and_then(|r| r.read()) {
        Ok(r) => Setting::of(&r),
        Err(e) => return Some(format!("benchrun: deep idle cannot be recorded on the servers' CPUs: {e}")),
    };
    let on = s.deep_enabled_on();
    (!on.is_empty()).then(|| {
        format!(
            "benchrun: deep idle states ({}) are enabled on the servers' CPUs {}, so fixed-rate tails include \
             waking from them (docs/research/measuring.md, \"A tail at one fixed rate can rank the CPU's idle \
             states\"); `scripts/deep-idle off` disables them",
            s.deep_names().join(", "),
            list(&on)
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// A directory shaped like /sys/devices/system/cpu, removed when dropped.
    struct Fixture(PathBuf);

    /// This host's states: name, exit latency.
    const STATES: [(&str, u64); 4] = [("POLL", 0), ("C1", 1), ("C2", 18), ("C3", 350)];

    impl Fixture {
        fn new(tag: &str) -> Fixture {
            let p = std::env::temp_dir().join(format!("benchrun-cpuidle-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&p);
            std::fs::create_dir_all(&p).unwrap();
            Fixture(p)
        }

        fn cpu(&self, c: usize, states: &[(&str, u64)]) -> &Fixture {
            for (k, (name, lat)) in states.iter().enumerate() {
                let d = self.0.join(format!("cpu{c}/cpuidle/state{k}"));
                std::fs::create_dir_all(&d).unwrap();
                std::fs::write(d.join("name"), format!("{name}\n")).unwrap();
                std::fs::write(d.join("latency"), format!("{lat}\n")).unwrap();
                self.set(c, k, 0, 0);
                std::fs::write(d.join("disable"), "0\n").unwrap();
            }
            self
        }

        fn set(&self, c: usize, k: usize, usage: u64, time: u64) {
            let d = self.0.join(format!("cpu{c}/cpuidle/state{k}"));
            std::fs::write(d.join("usage"), format!("{usage}\n")).unwrap();
            std::fs::write(d.join("time"), format!("{time}\n")).unwrap();
        }

        fn disable(&self, c: usize, k: usize, off: bool) {
            let d = self.0.join(format!("cpu{c}/cpuidle/state{k}"));
            std::fs::write(d.join("disable"), if off { "1\n" } else { "0\n" }).unwrap();
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn counts_a_window() {
        let f = Fixture::new("window");
        f.cpu(0, &STATES).cpu(1, &STATES);
        f.set(0, 2, 100, 5_000);
        f.set(1, 3, 7, 90_000);
        let r = Reader::open(&f.0, &[0, 1]).unwrap();
        let a = r.read().unwrap();
        assert_eq!(
            a.cpus[0].1[2],
            StateCount { name: "C2".into(), latency_us: 18, usage: 100, time_us: 5_000, disabled: false }
        );
        f.set(0, 1, 500, 2_000);
        f.set(0, 2, 130, 6_000);
        f.set(1, 2, 12, 400);
        f.set(1, 3, 10, 95_000);
        let w = Window::between(&a, &r.read().unwrap()).unwrap();
        let by = |n: &str| w.states.iter().find(|s| s.name == n).unwrap().clone();
        assert_eq!((by("C1").entries, by("C1").time_us), (500, 2_000));
        assert_eq!((by("C2").entries, by("C2").time_us), (30 + 12, 1_000 + 400));
        assert_eq!((by("C3").entries, by("C3").time_us), (3, 5_000));
        assert_eq!(w.deep_entries(), 45);
        // 45 deep entries over 1500 requests: 30 a thousand.
        assert_eq!(w.deep_per_kreq(1500.0), Some(30.0));
        assert_eq!(w.deep_per_kreq(0.0), None);
        let j = w.to_json(1500.0);
        assert_eq!(per_kreq(&j), Some(30.0));
        assert_eq!(j["cpus"], "0,1");
        assert_eq!(j["states"][3]["latency_us"], 350);

        // A group of two segments adds up.
        let mut g = w.clone();
        g.add(&w).unwrap();
        assert_eq!(g.deep_entries(), 90);
    }

    #[test]
    fn deep_is_by_latency_not_name() {
        let f = Fixture::new("latency");
        f.cpu(0, &[("POLL", 0), ("C1", 2), ("C1E", 4), ("C6", 170)]);
        let r = Reader::open(&f.0, &[0]).unwrap();
        let a = r.read().unwrap();
        for k in 0..4 {
            f.set(0, k, 10, 10);
        }
        let w = Window::between(&a, &r.read().unwrap()).unwrap();
        assert_eq!(w.deep_entries(), 10);
        assert_eq!(Setting::of(&a).deep_names(), vec!["C6"]);
    }

    #[test]
    fn missing_counters_say_why() {
        let f = Fixture::new("missing");
        f.cpu(0, &STATES);
        let e = Reader::open(&f.0, &[0, 1]).err().unwrap();
        assert!(e.contains("CPU 1"), "{e}");
        let j = setting_json(&f.0, &[0, 1]);
        assert!(j["absent"].as_str().unwrap().contains("CPU 1"));
        assert!(warning(&f.0, &[1]).unwrap().contains("cannot be recorded"));
        // A record from before the runner kept the counters, and one that says they were
        // absent, both read as not recorded.
        assert_eq!(per_kreq(&Value::Null), None);
        assert_eq!(per_kreq(&not_recorded("absent")), None);
    }

    #[test]
    fn a_counter_that_goes_backwards_is_not_recorded() {
        let f = Fixture::new("backwards");
        f.cpu(0, &STATES);
        f.set(0, 2, 1_000_000, 50_000_000);
        let r = Reader::open(&f.0, &[0]).unwrap();
        let a = r.read().unwrap();
        // Offline and back: the counters start again from zero.
        f.set(0, 2, 40, 900);
        let e = Window::between(&a, &r.read().unwrap()).unwrap_err();
        assert!(e.contains("backwards"), "{e}");
        // Wrapped: lower than before, the same way.
        f.set(0, 2, u64::MAX, u64::MAX);
        let b = r.read().unwrap();
        f.set(0, 2, 3, 3);
        assert!(Window::between(&b, &r.read().unwrap()).is_err());
    }

    #[test]
    fn a_setting_changed_in_the_window_is_not_recorded() {
        let f = Fixture::new("toggle");
        f.cpu(0, &STATES).cpu(1, &STATES);
        let r = Reader::open(&f.0, &[0, 1]).unwrap();
        let a = r.read().unwrap();
        assert_eq!(Setting::of(&a).deep_enabled_on(), vec![0, 1]);
        assert!(warning(&f.0, &[0, 1]).unwrap().contains("C2, C3"));
        f.disable(1, 2, true);
        f.disable(1, 3, true);
        assert!(Window::between(&a, &r.read().unwrap()).unwrap_err().contains("enabled or disabled"));
        // Disabled throughout: counted, and the record says where.
        let b = r.read().unwrap();
        let w = Window::between(&b, &r.read().unwrap()).unwrap();
        assert_eq!(w.states[2].disabled_on, vec![1]);
        assert_eq!(Setting::of(&b).deep_enabled_on(), vec![0]);
        f.disable(0, 2, true);
        f.disable(0, 3, true);
        assert_eq!(warning(&f.0, &[0, 1]), None);
    }

    #[test]
    fn confounded_needs_both_ratio_and_floor() {
        // The Connect h2 runs: cralloc in its slow mode against jemalloc.
        assert!(confounded(45.1, 21.2));
        assert!(confounded(21.2, 26.0));
        // The order service's runs, whose tails differ for another reason.
        assert!(!confounded(83.5, 91.1));
        assert!(!confounded(19.9, 21.2));
        // Near zero the ratio alone says nothing.
        assert!(!confounded(0.1, 0.5));
        assert!(confounded(0.0, 1.5));
    }
}
