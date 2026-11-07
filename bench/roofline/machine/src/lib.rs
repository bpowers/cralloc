//! What the machine probes share: pinning, the generic timer, per-thread counters on the
//! dynamic PMU type, mappings on small or huge pages, and the pointer chase.
//!
//! Any aarch64 Linux: pages, huge pages, the timer's rate, PMUs and CPU sets are read from the
//! kernel and sysfs. A heterogeneous machine has one PMU for each kind of core, so a counter is
//! opened on the PMU of the CPU the thread is pinned to, not on `PERF_TYPE_HARDWARE`.
#![cfg(target_arch = "aarch64")]

use std::arch::asm;
use std::ffi::c_void;

unsafe extern "C" {
    pub fn mmap(addr: *mut u8, len: usize, prot: i32, flags: i32, fd: i32, off: i64) -> *mut u8;
    pub fn munmap(addr: *mut u8, len: usize) -> i32;
    pub fn madvise(addr: *mut u8, len: usize, advice: i32) -> i32;
    pub fn mprotect(addr: *mut u8, len: usize, prot: i32) -> i32;
    pub fn mincore(addr: *mut u8, len: usize, vec: *mut u8) -> i32;
    fn sched_setaffinity(pid: i32, size: usize, mask: *const u64) -> i32;
    pub fn syscall(n: i64, ...) -> i64;
    fn read(fd: i32, buf: *mut c_void, n: usize) -> isize;
    fn ioctl(fd: i32, req: u64, arg: u64) -> i32;
    fn close(fd: i32) -> i32;
    fn __errno_location() -> *mut i32;
    fn sysconf(name: i32) -> i64;
}

pub const PROT_RW: i32 = 3;
pub const MAP_PRIVATE_ANON: i32 = 0x22;
pub const MAP_NORESERVE: i32 = 0x4000;
pub const MADV_DONTNEED: i32 = 4;
pub const MADV_FREE: i32 = 8;
pub const MADV_HUGEPAGE: i32 = 14;
pub const MADV_NOHUGEPAGE: i32 = 15;
pub const MADV_COLD: i32 = 20;
pub const MADV_POPULATE_WRITE: i32 = 23;
pub const MADV_COLLAPSE: i32 = 25;

pub const SYS_FUTEX: i64 = 98;
pub const SYS_PERF_EVENT_OPEN: i64 = 241;
pub const SYS_MEMBARRIER: i64 = 283;
pub const SYS_RSEQ: i64 = 293;

pub fn errno() -> i32 {
    // SAFETY: the C library's thread-local errno.
    unsafe { *__errno_location() }
}

/// Bytes in a page, from the kernel.
pub fn page() -> usize {
    const SC_PAGESIZE: i32 = 30;
    // SAFETY: no arguments to get wrong.
    unsafe { sysconf(SC_PAGESIZE) as usize }
}

/// Bytes in a transparent huge page, from the kernel.
pub fn huge() -> usize {
    std::fs::read_to_string("/sys/kernel/mm/transparent_hugepage/hpage_pmd_size")
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(32 << 20)
}

pub fn pin(cpu: usize) {
    let mut mask = [0u64; 16];
    mask[cpu / 64] |= 1 << (cpu % 64);
    // SAFETY: a valid CPU set of the size passed, for the calling thread.
    let r = unsafe { sched_setaffinity(0, size_of_val(&mask), mask.as_ptr()) };
    assert_eq!(r, 0, "pin to CPU {cpu}");
}

/// The generic timer, ordered against the instructions before it.
#[inline(always)]
pub fn ticks() -> u64 {
    let t: u64;
    // SAFETY: reading the virtual count is allowed at EL0 and touches no memory.
    unsafe { asm!("isb", "mrs {t}, cntvct_el0", t = out(reg) t, options(nostack, nomem, preserves_flags)) };
    t
}

/// Ticks of [`ticks`] a second.
pub fn tick_rate() -> u64 {
    let f: u64;
    // SAFETY: as for [`ticks`].
    unsafe { asm!("mrs {f}, cntfrq_el0", f = out(reg) f, options(nostack, nomem, preserves_flags)) };
    f
}

pub fn ns_of(ticks: u64) -> f64 {
    ticks as f64 * 1e9 / tick_rate() as f64
}

/// The core PMU that serves `cpu`, from sysfs: its type number and the codes of its cycle and
/// instruction events. Apple's names them `cycles` and `instructions`, Arm's PMUv3 `cpu_cycles` and
/// `inst_retired`. A device with no `cpus` file is not a core PMU.
fn pmu_of(cpu: usize) -> Option<(u64, u64, u64)> {
    for entry in std::fs::read_dir("/sys/bus/event_source/devices").ok()?.flatten() {
        let path = entry.path();
        let Ok(list) = std::fs::read_to_string(path.join("cpus")) else {
            continue;
        };
        if !parse_cpus(list.trim()).contains(&cpu) {
            continue;
        }
        let code = |names: [&str; 2]| {
            names.iter().find_map(|name| {
                let text = std::fs::read_to_string(path.join("events").join(name)).ok()?;
                let hex = text.trim().strip_prefix("event=0x")?;
                u64::from_str_radix(hex, 16).ok()
            })
        };
        let (Some(cycles), Some(instructions)) =
            (code(["cycles", "cpu_cycles"]), code(["instructions", "inst_retired"]))
        else {
            continue;
        };
        let ty = std::fs::read_to_string(path.join("type")).ok()?.trim().parse().ok()?;
        return Some((ty, cycles, instructions));
    }
    None
}

pub fn parse_cpus(s: &str) -> Vec<usize> {
    let mut v = vec![];
    for part in s.split(',') {
        match part.split_once('-') {
            Some((a, b)) => v.extend(a.parse::<usize>().unwrap()..=b.parse().unwrap()),
            None => v.push(part.parse().unwrap()),
        }
    }
    v
}

/// Which event a [`Counter`] counts.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Event {
    Cycles,
    Instructions,
}

/// A counter on the calling thread, which must be pinned to the CPU it is opened for. Where the
/// kernel gives none, as in a virtual machine, it reads zero, and the probes' nanoseconds still
/// hold.
pub struct Counter(Option<i32>);

impl Counter {
    pub fn open(cpu: usize, event: Event, user_only: bool) -> Counter {
        // Without a listing, the generic hardware events, which a homogeneous machine serves.
        let (ty, cycles, instructions) = pmu_of(cpu).unwrap_or((0, 0, 1));
        // struct perf_event_attr, 128 bytes.
        let mut attr = [0u64; 16];
        attr[0] = ty | (128u64 << 32);
        attr[1] = match event {
            Event::Cycles => cycles,
            Event::Instructions => instructions,
        };
        let disabled = 1u64;
        let exclude_kernel = 1u64 << 5;
        let exclude_hv = 1u64 << 6;
        // Apple's PMU refuses an event that does not exclude the guest.
        let exclude_guest = 1u64 << 20;
        attr[5] = disabled | exclude_hv | exclude_guest | if user_only { exclude_kernel } else { 0 };
        // SAFETY: attr is a perf_event_attr of the size it declares.
        let fd = unsafe { syscall(SYS_PERF_EVENT_OPEN, attr.as_ptr(), 0i32, -1i32, -1i32, 8u64) };
        if fd < 0 {
            eprintln!("no counter on CPU {cpu}: errno {}", errno());
            return Counter(None);
        }
        let fd = fd as i32;
        // SAFETY: PERF_EVENT_IOC_ENABLE on our own descriptor.
        unsafe { ioctl(fd, 0x2400, 0) };
        Counter(Some(fd))
    }

    pub fn cycles(cpu: usize) -> Counter {
        Counter::open(cpu, Event::Cycles, false)
    }

    pub fn read(&self) -> u64 {
        let Some(fd) = self.0 else {
            return 0;
        };
        let mut v = 0u64;
        // SAFETY: reads 8 bytes into v.
        let n = unsafe { read(fd, (&raw mut v).cast(), 8) };
        assert_eq!(n, 8);
        v
    }
}

impl Drop for Counter {
    fn drop(&mut self) {
        if let Some(fd) = self.0 {
            // SAFETY: our own descriptor.
            unsafe { close(fd) };
        }
    }
}

/// Which pages a mapping asks for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Pages {
    Small,
    Huge,
}

/// A private anonymous mapping aligned to a huge page.
pub struct Map {
    raw: *mut u8,
    raw_len: usize,
    pub p: *mut u8,
    pub len: usize,
}

// SAFETY: a plain address range; what threads do with it is each caller's business.
unsafe impl Send for Map {}
unsafe impl Sync for Map {}

impl Map {
    /// Maps `len` bytes, advised for `pages`, nothing resident.
    pub fn new(len: usize, pages: Pages) -> Map {
        let m = Map::bare(len);
        let advice = match pages {
            Pages::Small => MADV_NOHUGEPAGE,
            Pages::Huge => MADV_HUGEPAGE,
        };
        assert_eq!(adv(m.p, m.len, advice), 0);
        m
    }

    /// Maps `len` bytes with no advice, nothing resident.
    pub fn bare(len: usize) -> Map {
        let h = huge();
        let raw_len = len + h;
        // SAFETY: a fresh mapping at an address of the kernel's choosing.
        let raw = unsafe { mmap(std::ptr::null_mut(), raw_len, PROT_RW, MAP_PRIVATE_ANON | MAP_NORESERVE, -1, 0) };
        assert!(raw as isize != -1, "mmap: errno {}", errno());
        let p = raw.map_addr(|a| (a + h - 1) & !(h - 1));
        Map { raw, raw_len, p, len }
    }

    /// Writes one byte of every page.
    pub fn touch(&self) {
        let step = page();
        let mut o = 0;
        while o < self.len {
            // SAFETY: inside the mapping.
            unsafe { self.p.add(o).write_volatile(0) };
            o += step;
        }
    }

    /// Maps and touches.
    pub fn resident(len: usize, pages: Pages) -> Map {
        let m = Map::new(len, pages);
        m.touch();
        m
    }
}

impl Drop for Map {
    fn drop(&mut self) {
        // SAFETY: the whole of what `bare` mapped.
        unsafe { munmap(self.raw, self.raw_len) };
    }
}

pub fn adv(p: *mut u8, len: usize, advice: i32) -> i32 {
    // SAFETY: the kernel checks the range; callers pass parts of their own mappings.
    if unsafe { madvise(p, len, advice) } != 0 { errno() } else { 0 }
}

pub fn smaps(key: &str) -> u64 {
    let s = std::fs::read_to_string("/proc/self/smaps_rollup").unwrap_or_default();
    s.lines()
        .find(|l| l.starts_with(key))
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|v| v.parse().ok())
        .unwrap_or(0)
}

pub fn vmstat(key: &str) -> u64 {
    let s = std::fs::read_to_string("/proc/vmstat").unwrap_or_default();
    s.lines()
        .find(|l| l.split_whitespace().next() == Some(key))
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|v| v.parse().ok())
        .unwrap_or(0)
}

/// MiB of the process's memory that huge pages back.
pub fn huge_mib() -> u64 {
    smaps("AnonHugePages:") >> 10
}

/// A small deterministic generator.
pub struct Rng(pub u64);

impl Rng {
    pub fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    pub fn shuffle<T>(&mut self, v: &mut [T]) {
        for i in (1..v.len()).rev() {
            let j = (self.next() % (i as u64 + 1)) as usize;
            v.swap(i, j);
        }
    }
}

/// Follows `loads` pointers from `p`, unrolled so that the loop's branch is a small share.
#[inline(never)]
pub fn chase(mut p: *const usize, loads: usize) -> *const usize {
    for _ in 0..loads.div_ceil(16) {
        for _ in 0..16 {
            // SAFETY: the caller laid a cycle of pointers from `p`.
            p = unsafe { p.read_volatile() } as *const usize;
        }
    }
    p
}

/// Lays one cycle through the addresses `at`, in that order, and returns its start.
pub fn link(at: &[*mut u8]) -> *const usize {
    for (i, &a) in at.iter().enumerate() {
        let next = at[(i + 1) % at.len()];
        // SAFETY: each address is a word-aligned place in a live mapping of the caller's.
        unsafe { a.cast::<usize>().write_volatile(next as usize) };
    }
    at[0].cast()
}

/// One timing of a chase: cycles and nanoseconds a load.
#[derive(Clone, Copy, Debug)]
pub struct Timed {
    pub cycles: f64,
    pub ns: f64,
}

/// Runs the chase from `start` for `warm` loads, then `rounds` times `loads` loads, and returns the
/// fastest round.
pub fn time_chase(start: *const usize, loads: usize, warm: usize, rounds: usize, cycles: &Counter) -> Timed {
    let mut p = chase(start, warm);
    let mut best = Timed { cycles: f64::MAX, ns: f64::MAX };
    for _ in 0..rounds {
        let (t0, c0) = (ticks(), cycles.read());
        p = chase(p, loads);
        let (c1, t1) = (cycles.read(), ticks());
        let ns = ns_of(t1 - t0) / loads as f64;
        if ns < best.ns {
            best = Timed { cycles: (c1 - c0) as f64 / loads as f64, ns };
        }
    }
    std::hint::black_box(p);
    best
}

/// Bytes in a line of the CPU's first-level data cache as sysfs tells it, or 128 where it does not
/// (Apple's kernel leaves it out). `RF_LINE` overrides it. Used only to lay out probes: `lines`
/// measures the real unit.
pub fn line(cpu: usize) -> usize {
    if let Some(l) = std::env::var("RF_LINE").ok().and_then(|s| s.parse().ok()) {
        return l;
    }
    let path = format!("/sys/devices/system/cpu/cpu{cpu}/cache/index0/coherency_line_size");
    std::fs::read_to_string(path).ok().and_then(|s| s.trim().parse().ok()).filter(|&l| l > 0).unwrap_or(128)
}

pub fn median(v: &mut [f64]) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let n = v.len();
    if n % 2 == 1 { v[n / 2] } else { (v[n / 2 - 1] + v[n / 2]) / 2.0 }
}
