import os
import sys
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from sub import sub

# Diagnostic: when a claim of a large slot finds pages of it clean (returned to the OS, so the
# tenant faults them back), how long ago its region's last purge was, by whichever purger, and
# its last purge by the hold; bytes by log2 of the milliseconds, and by the class's size. At
# exit, to stderr: "REVIVAL any|hold ms_from M bytes B" and "REVIVAL class SIZE bytes B".
open("crates/asmalloc/src/os/linux/revival.rs", "w").write(r'''
//! DIAGNOSTIC ONLY, never committed: what large claims fault back, and how soon after a purge.
use super::syscall::{int, syscall};
use core::fmt::Write;
use core::sync::atomic::{AtomicU64, Ordering::Relaxed};

const REGIONS: usize = 4096;
static LAST: [AtomicU64; REGIONS] = [const { AtomicU64::new(0) }; REGIONS];
static LAST_HOLD: [AtomicU64; REGIONS] = [const { AtomicU64::new(0) }; REGIONS];
// [any, hold][log2 of ms + 1, 0 for never]
static HIST: [[AtomicU64; 40]; 2] = [const { [const { AtomicU64::new(0) }; 40] }; 2];
static BY_SIZE: [AtomicU64; 64] = [const { AtomicU64::new(0) }; 64];
static START_TSC: AtomicU64 = AtomicU64::new(0);
static START_NS: AtomicU64 = AtomicU64::new(0);

fn now() -> u64 {
    // SAFETY: rdtsc has no preconditions.
    unsafe { core::arch::x86_64::_rdtsc() }
}

fn mono() -> u64 {
    let mut ts = [0u64; 2];
    // SAFETY: clock_gettime into a live local buffer.
    unsafe {
        syscall(228, [int(1), ts.as_mut_ptr().cast(), int(0), int(0), int(0), int(0)]);
    }
    ts[0] * 1_000_000_000 + ts[1]
}

fn start() {
    if START_TSC.load(Relaxed) == 0 && START_TSC.compare_exchange(0, now(), Relaxed, Relaxed).is_ok() {
        START_NS.store(mono(), Relaxed);
    }
}

pub(crate) fn purged(region: usize, hold: bool) {
    start();
    let t = now();
    if let Some(w) = LAST.get(region) {
        w.store(t, Relaxed);
    }
    if hold && let Some(w) = LAST_HOLD.get(region) {
        w.store(t, Relaxed);
    }
}

/// Ticks per millisecond, about: the cycle counter runs at a few GHz.
const TICKS_PER_MS: u64 = 4_000_000;

fn bucket(last: u64, t: u64) -> usize {
    if last == 0 {
        return 0;
    }
    let ms = t.saturating_sub(last) / TICKS_PER_MS;
    (1 + (64 - ms.leading_zeros()) as usize).min(39)
}

pub(crate) fn revived(region: usize, bytes: usize, size: usize) {
    if bytes == 0 {
        return;
    }
    start();
    let t = now();
    let any = LAST.get(region).map_or(0, |w| w.load(Relaxed));
    let hold = LAST_HOLD.get(region).map_or(0, |w| w.load(Relaxed));
    HIST[0][bucket(any, t)].fetch_add(bytes as u64, Relaxed);
    HIST[1][bucket(hold, t)].fetch_add(bytes as u64, Relaxed);
    BY_SIZE[(63 - size.max(1).leading_zeros()) as usize].fetch_add(bytes as u64, Relaxed);
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
    emit(|b| {
        let _ = writeln!(b, "REVIVAL ticks_per_ms {}", tsc * 1_000_000 / ns);
    });
    for (k, name) in ["any", "hold"].iter().enumerate() {
        for (i, w) in HIST[k].iter().enumerate() {
            let v = w.load(Relaxed);
            if v != 0 {
                let from: i64 = if i == 0 { -1 } else { (1i64 << (i - 1)) - 1 };
                emit(|b| {
                    let _ = writeln!(b, "REVIVAL {name} ms_from {from} bytes {v}");
                });
            }
        }
    }
    for (i, w) in BY_SIZE.iter().enumerate() {
        let v = w.load(Relaxed);
        if v != 0 {
            emit(|b| {
                let _ = writeln!(b, "REVIVAL class_log2 {i} bytes {v}");
            });
        }
    }
}

#[used]
#[unsafe(link_section = ".fini_array")]
static FINI: extern "C" fn() = dump;
''')
sub("crates/asmalloc/src/os/linux/mod.rs", "mod syscall;\n", "mod syscall;\npub(crate) mod revival;\n")
R = "crates/asmalloc/src/back/region.rs"
# Every purge of a unit, by any purger.
sub(R, "    release_range(host, region, first, last, low, high + 1);\n    (Purge::Done(cleaned * page), read)\n",
    "    crate::os::linux::revival::purged(region.index(), false);\n"
    "    release_range(host, region, first, last, low, high + 1);\n    (Purge::Done(cleaned * page), read)\n")
# A large claim that found clean pages.
# Only the clean pages under the new block: those it will fault back.
sub(R, "    let end = extent_of(layout, slot, unit, size, false);\n    let mut clean = 0usize;\n",
    "    let end = extent_of(layout, slot, unit, size, false);\n    let mut clean = 0usize;\n    let mut under = 0usize;\n")
sub(R, "        let inside = mask & below(w, end);\n        if seen & inside != 0 {\n            word.fetch_and(!inside);\n        }\n    }\n    Settled {\n",
    "        let inside = mask & below(w, end);\n        under += count(seen & inside);\n        if seen & inside != 0 {\n            word.fetch_and(!inside);\n        }\n    }\n"
    "    crate::os::linux::revival::revived(region.index(), under * layout.page_size(), layout.size());\n"
    "    Settled {\n")
H = "crates/asmalloc/src/back/hold.rs"
sub(H, "        let (bytes, read, done) = self.purge_region(class, region, budget, limit, true, Scope::All);\n",
    "        let (bytes, read, done) = self.purge_region(class, region, budget, limit, true, Scope::All);\n"
    "        if bytes != 0 {\n            crate::os::linux::revival::purged(region.index(), true);\n        }\n")
# Small classes: the pages a claim cleaned, which its fill or allocation faults back.
sub(R, "    let claimed = bits.count_ones() as usize * layout.size();\n    Settled {\n",
    "    crate::os::linux::revival::revived(region.index(), cleaned * page, layout.size());\n"
    "    let claimed = bits.count_ones() as usize * layout.size();\n    Settled {\n")
