import os
import sys
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from sub import sub

# Diagnostic: times the purge work a request's slow path does, each heartbeat's (`Back::
# heartbeat_with`: the clocked pass's step and the hold's look) and each payment of growth
# over the cap (`Back::pay_growth`), and prints at exit, to stderr, for each: how many, the
# longest, and a histogram by log2 of nanoseconds: "STEPTIME ...". For each step of 250 us
# or more it also reads the thread's CPU time, so that a long step can be told apart from one
# the thread spent off its CPU: "STEPTIME NAME long N wall_ns W cpu_ns C".
open("crates/asmalloc/src/os/linux/steptime.rs", "w").write(r'''
//! DIAGNOSTIC ONLY, never committed: times the purge work on slow paths.
use super::syscall::{int, syscall};
use core::fmt::Write;
use core::sync::atomic::{AtomicU64, Ordering::Relaxed};

static COUNT: [AtomicU64; 2] = [const { AtomicU64::new(0) }; 2];
static MAX: [AtomicU64; 2] = [const { AtomicU64::new(0) }; 2];
static HIST: [[AtomicU64; 64]; 2] = [const { [const { AtomicU64::new(0) }; 64] }; 2];
static LONG: [AtomicU64; 2] = [const { AtomicU64::new(0) }; 2];
static LONG_WALL: [AtomicU64; 2] = [const { AtomicU64::new(0) }; 2];
static LONG_CPU: [AtomicU64; 2] = [const { AtomicU64::new(0) }; 2];
static START_TSC: AtomicU64 = AtomicU64::new(0);
static START_NS: AtomicU64 = AtomicU64::new(0);

#[inline(always)]
pub(crate) fn now() -> u64 {
    // SAFETY: rdtsc has no preconditions.
    unsafe { core::arch::x86_64::_rdtsc() }
}

fn clock(id: usize) -> u64 {
    let mut ts = [0u64; 2];
    // SAFETY: clock_gettime into a live local buffer.
    unsafe {
        syscall(228, [int(id), ts.as_mut_ptr().cast(), int(0), int(0), int(0), int(0)]);
    }
    ts[0] * 1_000_000_000 + ts[1]
}

fn mono() -> u64 {
    clock(1)
}

/// The calling thread's CPU time.
pub(crate) fn cpu() -> u64 {
    clock(3)
}

/// For a step that took `ticks` and `cpu_ns` of its thread's CPU time.
pub(crate) fn note_long(kind: usize, ticks: u64, cpu_ns: u64) {
    LONG[kind].fetch_add(1, Relaxed);
    LONG_WALL[kind].fetch_add(ticks, Relaxed);
    LONG_CPU[kind].fetch_add(cpu_ns, Relaxed);
}

/// Ticks of about 250 us at 4 GHz and more.
pub(crate) const LONG_TICKS: u64 = 1_000_000;

pub(crate) fn note(kind: usize, ticks: u64) {
    if START_TSC.load(Relaxed) == 0 && START_TSC.compare_exchange(0, now(), Relaxed, Relaxed).is_ok() {
        START_NS.store(mono(), Relaxed);
    }
    COUNT[kind].fetch_add(1, Relaxed);
    MAX[kind].fetch_max(ticks, Relaxed);
    HIST[kind][63 - ticks.max(1).leading_zeros() as usize].fetch_add(1, Relaxed);
}

struct Buf {
    b: [u8; 256],
    n: usize,
}

impl Write for Buf {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        for &c in s.as_bytes() {
            if self.n < self.b.len() {
                self.b[self.n] = c;
                self.n += 1;
            }
        }
        Ok(())
    }
}

fn emit(f: impl FnOnce(&mut Buf)) {
    let mut b = Buf { b: [0; 256], n: 0 };
    f(&mut b);
    // SAFETY: write(2) from a live local buffer.
    unsafe {
        syscall(1, [int(2), b.b.as_mut_ptr(), int(b.n), int(0), int(0), int(0)]);
    }
}

extern "C" fn dump() {
    let tsc = now().saturating_sub(START_TSC.load(Relaxed)).max(1);
    let ns = mono().saturating_sub(START_NS.load(Relaxed)).max(1);
    let per = ns * 1000 / tsc;
    for (kind, name) in ["beat", "growth"].iter().enumerate() {
        emit(|b| {
            let _ = writeln!(
                b,
                "STEPTIME {name} count {} max_ns {}",
                COUNT[kind].load(Relaxed),
                MAX[kind].load(Relaxed) * per / 1000
            );
        });
        emit(|b| {
            let _ = writeln!(
                b,
                "STEPTIME {name} long {} wall_ns {} cpu_ns {}",
                LONG[kind].load(Relaxed),
                LONG_WALL[kind].load(Relaxed) * per / 1000,
                LONG_CPU[kind].load(Relaxed)
            );
        });
        for (k, w) in HIST[kind].iter().enumerate() {
            let v = w.load(Relaxed);
            if v != 0 {
                emit(|b| {
                    let _ = writeln!(b, "STEPTIME {name} hist_ns_from {} n {}", (1u64 << k) * per / 1000, v);
                });
            }
        }
    }
}

#[used]
#[unsafe(link_section = ".fini_array")]
static FINI: extern "C" fn() = dump;
''')
sub("crates/asmalloc/src/os/linux/mod.rs", "mod syscall;\n", "mod syscall;\npub(crate) mod steptime;\n")
P = "crates/asmalloc/src/back/purge.rs"
sub(P, "    pub fn heartbeat_with(&self, now: &dyn Fn() -> u64) {\n",
    "    pub fn heartbeat_with(&self, now: &dyn Fn() -> u64) {\n"
    "        let c0 = crate::os::linux::steptime::cpu();\n"
    "        let t0 = crate::os::linux::steptime::now();\n"
    "        self.heartbeat_with_timed(now);\n"
    "        let t = crate::os::linux::steptime::now() - t0;\n"
    "        crate::os::linux::steptime::note(0, t);\n"
    "        if t >= crate::os::linux::steptime::LONG_TICKS {\n"
    "            crate::os::linux::steptime::note_long(0, t, crate::os::linux::steptime::cpu() - c0);\n"
    "        }\n"
    "    }\n\n"
    "    fn heartbeat_with_timed(&self, now: &dyn Fn() -> u64) {\n")
sub(P, "    fn pay_growth(&self, step: usize) {\n",
    "    fn pay_growth(&self, step: usize) {\n"
    "        let c0 = crate::os::linux::steptime::cpu();\n"
    "        let t0 = crate::os::linux::steptime::now();\n"
    "        self.pay_growth_timed(step);\n"
    "        let t = crate::os::linux::steptime::now() - t0;\n"
    "        crate::os::linux::steptime::note(1, t);\n"
    "        if t >= crate::os::linux::steptime::LONG_TICKS {\n"
    "            crate::os::linux::steptime::note_long(1, t, crate::os::linux::steptime::cpu() - c0);\n"
    "        }\n"
    "    }\n\n"
    "    fn pay_growth_timed(&self, step: usize) {\n")
