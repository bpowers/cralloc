import os
import sys
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from sub import sub

# Diagnostic: each growth payment's walk (`Back::pay_growth`): the bitmap words it read, the
# regions it finished, what it returned and how long it took. At exit, to stderr, by log2 of
# the words read: "GROWTHWALK words_from W payments N dry D ticks T max_ticks M regions R
# bytes B", where dry counts the payments that returned nothing.
open("crates/asmalloc/src/os/linux/growthwalk.rs", "w").write(r'''
//! DIAGNOSTIC ONLY, never committed: growth payments' walks.
use super::syscall::{int, syscall};
use core::fmt::Write;
use core::sync::atomic::{AtomicU64, Ordering::Relaxed};

const B: usize = 20;
static N: [AtomicU64; B] = [const { AtomicU64::new(0) }; B];
static DRY: [AtomicU64; B] = [const { AtomicU64::new(0) }; B];
static TICKS: [AtomicU64; B] = [const { AtomicU64::new(0) }; B];
static MAX: [AtomicU64; B] = [const { AtomicU64::new(0) }; B];
static REGIONS: [AtomicU64; B] = [const { AtomicU64::new(0) }; B];
static BYTES: [AtomicU64; B] = [const { AtomicU64::new(0) }; B];
static OWED: [AtomicU64; B] = [const { AtomicU64::new(0) }; B];

pub(crate) fn now() -> u64 {
    // SAFETY: rdtsc has no preconditions.
    unsafe { core::arch::x86_64::_rdtsc() }
}

pub(crate) fn note(words: usize, regions: usize, bytes: usize, owed: usize, ticks: u64) {
    let b = ((usize::BITS - words.max(1).leading_zeros()) as usize).min(B - 1);
    N[b].fetch_add(1, Relaxed);
    DRY[b].fetch_add(u64::from(bytes == 0), Relaxed);
    TICKS[b].fetch_add(ticks, Relaxed);
    MAX[b].fetch_max(ticks, Relaxed);
    REGIONS[b].fetch_add(regions as u64, Relaxed);
    BYTES[b].fetch_add(bytes as u64, Relaxed);
    OWED[b].fetch_add(owed as u64, Relaxed);
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

extern "C" fn dump() {
    for i in 0..B {
        let n = N[i].load(Relaxed);
        if n == 0 {
            continue;
        }
        let mut b = Buf { b: [0; 256], n: 0 };
        let _ = writeln!(
            b,
            "GROWTHWALK words_from {} payments {} dry {} ticks {} max_ticks {} regions {} bytes {} owed {}",
            if i == 0 { 0 } else { 1usize << (i - 1) },
            n,
            DRY[i].load(Relaxed),
            TICKS[i].load(Relaxed),
            MAX[i].load(Relaxed),
            REGIONS[i].load(Relaxed),
            BYTES[i].load(Relaxed),
            OWED[i].load(Relaxed)
        );
        // SAFETY: write(2) from a live local buffer.
        unsafe {
            syscall(1, [int(2), b.b.as_mut_ptr(), int(b.n), int(0), int(0), int(0)]);
        }
    }
}

#[used]
#[unsafe(link_section = ".fini_array")]
static FINI: extern "C" fn() = dump;
''')
sub("crates/asmalloc/src/os/linux/mod.rs", "mod syscall;\n", "mod syscall;\npub(crate) mod growthwalk;\n")
P = "crates/asmalloc/src/back/purge.rs"
sub(P, "        let geometry = self.space.plan().geometry();\n        let debt = self.state.growth.swap(0, Relaxed);\n",
    "        let t0 = crate::os::linux::growthwalk::now();\n"
    "        let geometry = self.space.plan().geometry();\n        let debt = self.state.growth.swap(0, Relaxed);\n")
sub(P, "            self.state.growth.fetch_add(owed - purged, Relaxed);\n        }\n    }\n",
    "            self.state.growth.fetch_add(owed - purged, Relaxed);\n        }\n"
    "        if owed != 0 {\n"
    "            crate::os::linux::growthwalk::note(spent, finished, purged, owed, crate::os::linux::growthwalk::now() - t0);\n"
    "        }\n    }\n")
