import os
import sys
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from sub import sub

# Diagnostic: times each heartbeat's look at the hold on free large slots (`back::hold`), and
# prints at exit, to stderr, how many looks there were, how many returned memory, the bytes
# they returned, the longest look, and a histogram of the looks that returned memory by log2
# of their time in nanoseconds: "HOLDTIME ...".
open("crates/asmalloc/src/os/linux/holdtime.rs", "w").write(r'''
//! DIAGNOSTIC ONLY, never committed: times the hold's looks.
use super::syscall::{int, syscall};
use core::fmt::Write;
use core::sync::atomic::{AtomicU64, Ordering::Relaxed};

static LOOKS: AtomicU64 = AtomicU64::new(0);
static RETURNED: AtomicU64 = AtomicU64::new(0);
static BYTES: AtomicU64 = AtomicU64::new(0);
static MAX: AtomicU64 = AtomicU64::new(0);
static MAX_BYTES: AtomicU64 = AtomicU64::new(0);
pub(crate) static LAST: AtomicU64 = AtomicU64::new(0);
static HIST: [AtomicU64; 64] = [const { AtomicU64::new(0) }; 64];
static START_TSC: AtomicU64 = AtomicU64::new(0);
static START_NS: AtomicU64 = AtomicU64::new(0);

#[inline(always)]
pub(crate) fn now() -> u64 {
    // SAFETY: rdtsc has no preconditions.
    unsafe { core::arch::x86_64::_rdtsc() }
}

fn mono() -> u64 {
    let mut ts = [0u64; 2];
    // SAFETY: clock_gettime(CLOCK_MONOTONIC) into a live local buffer.
    unsafe {
        syscall(228, [int(1), ts.as_mut_ptr().cast(), int(0), int(0), int(0), int(0)]);
    }
    ts[0] * 1_000_000_000 + ts[1]
}

pub(crate) fn note(ticks: u64, bytes: usize) {
    if START_TSC.load(Relaxed) == 0 && START_TSC.compare_exchange(0, now(), Relaxed, Relaxed).is_ok() {
        START_NS.store(mono(), Relaxed);
    }
    LOOKS.fetch_add(1, Relaxed);
    if bytes != 0 {
        RETURNED.fetch_add(1, Relaxed);
        BYTES.fetch_add(bytes as u64, Relaxed);
        HIST[63 - ticks.max(1).leading_zeros() as usize].fetch_add(1, Relaxed);
    }
    if MAX.fetch_max(ticks, Relaxed) < ticks {
        MAX_BYTES.store(bytes as u64, Relaxed);
    }
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
    // Nanoseconds per thousand ticks.
    let per = ns * 1000 / tsc;
    emit(|b| {
        let _ = writeln!(
            b,
            "HOLDTIME looks {} returned {} bytes {} max_ns {} max_bytes {}",
            LOOKS.load(Relaxed),
            RETURNED.load(Relaxed),
            BYTES.load(Relaxed),
            MAX.load(Relaxed) * per / 1000,
            MAX_BYTES.load(Relaxed)
        );
    });
    for (k, w) in HIST.iter().enumerate() {
        let v = w.load(Relaxed);
        if v != 0 {
            emit(|b| {
                let _ = writeln!(b, "HOLDTIME hist_ns_from {} looks {}", (1u64 << k) * per / 1000, v);
            });
        }
    }
}

#[used]
#[unsafe(link_section = ".fini_array")]
static FINI: extern "C" fn() = dump;
''')
sub("crates/asmalloc/src/back/hold.rs",
    "        stopped && purged != 0 && purged < over\n",
    "        crate::os::linux::holdtime::LAST.store(purged as u64, Relaxed);\n"
    "        stopped && purged != 0 && purged < over\n")
sub("crates/asmalloc/src/os/linux/mod.rs", "mod syscall;\n", "mod syscall;\npub(crate) mod holdtime;\n")
sub("crates/asmalloc/src/back/hold.rs",
    "        let more = self.hold_look(count, reach(self));\n",
    "        let t0 = crate::os::linux::holdtime::now();\n"
    "        let more = self.hold_look(count, reach(self));\n"
    "        crate::os::linux::holdtime::note(\n"
    "            crate::os::linux::holdtime::now() - t0,\n"
    "            crate::os::linux::holdtime::LAST.swap(0, core::sync::atomic::Ordering::Relaxed) as usize,\n"
    "        );\n")
