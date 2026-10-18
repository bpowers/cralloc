//! Hardware and software counters through `perf_event_open`, counting a whole process.
//!
//! Counters are opened with `inherit`, so threads created afterwards are counted too, and a
//! read sums every thread, live or exited. Enabling or disabling a counter applies to its
//! inherited copies as well. Open them before spawning any thread.
use std::fs::File;
use std::io::{self, Read};
use std::os::fd::{AsRawFd, FromRawFd};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    /// A raw event of the core's PMU, in `perf_event_attr.config` encoding, under a name of
    /// the caller's choosing (see [`extra_events`]).
    Raw {
        name: &'static str,
        config: u64,
    },
    /// Core cycles in user and kernel mode.
    Cycles,
    /// Core cycles in user mode only.
    CyclesUser,
    /// Instructions retired in user and kernel mode.
    Instructions,
    PageFaults,
    /// CPU time in nanoseconds, summed over threads.
    TaskClock,
    ContextSwitches,
}

impl Event {
    pub fn name(self) -> &'static str {
        match self {
            Event::Cycles => "cycles",
            Event::CyclesUser => "cycles_user",
            Event::Instructions => "instructions",
            Event::PageFaults => "page_faults",
            Event::TaskClock => "task_clock_ns",
            Event::ContextSwitches => "ctx_switches",
            Event::Raw { name, .. } => name,
        }
    }

    fn type_config(self) -> (u32, u64) {
        const HARDWARE: u32 = 0;
        const SOFTWARE: u32 = 1;
        const RAW: u32 = 4;
        match self {
            Event::Raw { config, .. } => (RAW, config),
            Event::Cycles | Event::CyclesUser => (HARDWARE, 0),
            Event::Instructions => (HARDWARE, 1),
            Event::TaskClock => (SOFTWARE, 1),
            Event::PageFaults => (SOFTWARE, 2),
            Event::ContextSwitches => (SOFTWARE, 3),
        }
    }

    fn is_hardware(self) -> bool {
        matches!(self.type_config().0, 0 | 4)
    }
}

/// What a workload counts in-process. Three hardware counters: the runner's own two per
/// process and the NMI watchdog's one leave this many of Zen 5's six free.
pub const REGION_EVENTS: &[Event] = &[
    Event::Cycles,
    Event::CyclesUser,
    Event::Instructions,
    Event::PageFaults,
    Event::TaskClock,
    Event::ContextSwitches,
];

/// Extra raw events for the region counters, from `BENCH_EVENTS`: a comma-separated list of
/// `name=config`, the config in hex as `perf_event_attr.config` takes it for the core's PMU
/// (on AMD, `event | umask << 8`, with event bits 8 to 11 at bit 32). For example
/// `BENCH_EVENTS=dtlb_walk_4k=0x1045,dtlb_walk_2m=0x4045` counts page walks by page size.
/// They count user mode only.
///
/// The core has six counters and the NMI watchdog may hold one, so any extra event replaces
/// the user-cycles counter to make room for three; a run whose counters were multiplexed
/// says so in its result. For investigations: the runner never sets it.
pub fn extra_events() -> Vec<Event> {
    let Ok(list) = std::env::var("BENCH_EVENTS") else { return Vec::new() };
    list.split(',')
        .filter(|s| !s.is_empty())
        .map(|item| {
            let (name, config) =
                item.split_once('=').unwrap_or_else(|| panic!("BENCH_EVENTS item {item:?} is not name=config"));
            let hex = config.trim_start_matches("0x");
            let config =
                u64::from_str_radix(hex, 16).unwrap_or_else(|_| panic!("BENCH_EVENTS config {config:?} is not hex"));
            Event::Raw { name: Box::leak(name.to_string().into_boxed_str()), config }
        })
        .collect()
}

/// What the runner counts for a whole process from outside.
pub const PROCESS_EVENTS: &[Event] = &[Event::Cycles, Event::Instructions, Event::PageFaults, Event::TaskClock];

/// Whose events to count.
#[derive(Clone, Copy, Debug)]
pub enum Target {
    /// This process, starting disabled.
    Me,
    /// Another process, enabled when it next calls `execve`.
    ChildBeforeExec(i32),
}

struct Counter {
    event: Event,
    file: File,
}

impl Counter {
    fn open(event: Event, target: Target) -> io::Result<Counter> {
        let (ty, config) = event.type_config();
        // struct perf_event_attr, version 8 (128 bytes).
        let mut attr = [0u64; 16];
        attr[0] = ty as u64 | (128u64 << 32);
        attr[1] = config;
        // read_format: TOTAL_TIME_ENABLED | TOTAL_TIME_RUNNING, to detect multiplexing.
        attr[4] = 1 | 2;
        let disabled = 1 << 0;
        let inherit = 1 << 1;
        let exclude_kernel = 1 << 5;
        let exclude_hv = 1 << 6;
        let enable_on_exec = 1 << 12;
        let mut flags = disabled | inherit | exclude_hv;
        // Raw events count user mode only, so that the program's own memory behaviour is
        // apart from the kernel's page zeroing; kernel cycles show the latter.
        if event == Event::CyclesUser || matches!(event, Event::Raw { .. }) {
            flags |= exclude_kernel;
        }
        let pid = match target {
            Target::Me => 0,
            Target::ChildBeforeExec(pid) => {
                flags |= enable_on_exec;
                pid
            }
        };
        attr[5] = flags;
        const PERF_FLAG_FD_CLOEXEC: u64 = 8;
        // SAFETY: attr is a valid perf_event_attr of the size it declares.
        let fd =
            unsafe { libc::syscall(libc::SYS_perf_event_open, attr.as_ptr(), pid, -1i32, -1i32, PERF_FLAG_FD_CLOEXEC) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: the kernel just gave us this descriptor.
        Ok(Counter { event, file: unsafe { File::from_raw_fd(fd as i32) } })
    }

    fn ioctl(&self, request: u64) {
        // SAFETY: a perf ioctl with no argument on a perf descriptor.
        unsafe { libc::ioctl(self.file.as_raw_fd(), request as _, 0) };
    }

    /// (value, time enabled, time running)
    fn read(&self) -> io::Result<(u64, u64, u64)> {
        let mut b = [0u8; 24];
        (&self.file).read_exact(&mut b)?;
        let w = |i: usize| u64::from_ne_bytes(b[i * 8..i * 8 + 8].try_into().unwrap());
        Ok((w(0), w(1), w(2)))
    }
}

/// A set of counters over one process.
pub struct Counters(Vec<Counter>);

impl Counters {
    /// Opens `events` for `target`. Hardware events that cannot be opened (no PMU, or a
    /// restrictive `perf_event_paranoid`) are an error; so are software ones.
    pub fn open(events: &[Event], target: Target) -> io::Result<Counters> {
        events.iter().map(|&e| Counter::open(e, target)).collect::<io::Result<Vec<_>>>().map(Counters)
    }

    /// The in-process region counters, with any `BENCH_EVENTS` ([`extra_events`]). Panics
    /// with the reason if they cannot be opened.
    pub fn region() -> Counters {
        let extra = extra_events();
        let mut events: Vec<Event> =
            REGION_EVENTS.iter().copied().filter(|&e| extra.is_empty() || e != Event::CyclesUser).collect();
        events.extend(extra);
        Counters::open(&events, Target::Me).unwrap_or_else(|e| panic!("perf_event_open: {e}"))
    }

    pub fn reset(&self) {
        const RESET: u64 = 0x2403;
        self.0.iter().for_each(|c| c.ioctl(RESET));
    }

    pub fn enable(&self) {
        const ENABLE: u64 = 0x2400;
        self.0.iter().for_each(|c| c.ioctl(ENABLE));
    }

    pub fn disable(&self) {
        const DISABLE: u64 = 0x2401;
        self.0.iter().for_each(|c| c.ioctl(DISABLE));
    }

    pub fn read(&self) -> Sample {
        let mut s = Sample::default();
        for c in &self.0 {
            let Ok((v, enabled, running)) = c.read() else { continue };
            if c.event.is_hardware() && running < enabled {
                s.multiplexed = true;
            }
            s.values.push((c.event.name(), v));
        }
        s
    }
}

/// Counter values, by event name.
#[derive(Clone, Debug, Default)]
pub struct Sample {
    pub values: Vec<(&'static str, u64)>,
    /// A hardware counter was not running for all of the time it was enabled, because more
    /// counters were in use than the core has. Its value is then an estimate; treat the
    /// sample as suspect.
    pub multiplexed: bool,
}

impl Sample {
    pub fn get(&self, name: &str) -> Option<u64> {
        self.values.iter().find(|(n, _)| *n == name).map(|&(_, v)| v)
    }

    /// `self - earlier`, event by event.
    pub fn since(&self, earlier: &Sample) -> Sample {
        let values = self.values.iter().map(|&(n, v)| (n, v.saturating_sub(earlier.get(n).unwrap_or(0)))).collect();
        Sample { values, multiplexed: self.multiplexed || earlier.multiplexed }
    }

    pub fn to_json(&self) -> serde_json::Value {
        let mut m = serde_json::Map::new();
        for &(n, v) in &self.values {
            m.insert(n.to_string(), v.into());
        }
        m.insert("multiplexed".into(), self.multiplexed.into());
        serde_json::Value::Object(m)
    }
}
