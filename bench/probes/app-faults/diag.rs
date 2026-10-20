//! DIAGNOSTIC ONLY, never committed: counts memory system calls by call site and the
//! revival of purged pages, and prints them to stderr at exit.

use super::syscall::{int, nr, syscall};
use core::fmt::Write;
use core::panic::Location;
use core::ptr;
use core::sync::atomic::{AtomicPtr, AtomicU64, Ordering::Relaxed};

pub(crate) const MPROTECT: u64 = 0;
pub(crate) const DONTNEED: u64 = 1;
pub(crate) const HUGE: u64 = 2;
pub(crate) const NOHUGE: u64 = 3;
pub(crate) const MINCORE: u64 = 4;

struct Site {
    loc: AtomicPtr<Location<'static>>,
    kind: AtomicU64,
    calls: AtomicU64,
    bytes: AtomicU64,
    ticks: AtomicU64,
}

const N: usize = 256;
static SITES: [Site; N] = [const {
    Site {
        loc: AtomicPtr::new(ptr::null_mut()),
        kind: AtomicU64::new(u64::MAX),
        calls: AtomicU64::new(0),
        bytes: AtomicU64::new(0),
        ticks: AtomicU64::new(0),
    }
}; N];

static LAST_PURGE: [AtomicU64; 4096] = [const { AtomicU64::new(0) }; 4096];
// [small/large][log2 bucket]: bytes revived
static REVIVED: [[AtomicU64; 64]; 2] = [const { [const { AtomicU64::new(0) }; 64] }; 2];
static REVIVED_NEVER: [AtomicU64; 2] = [const { AtomicU64::new(0) }; 2];
static START: AtomicU64 = AtomicU64::new(0);

#[inline(always)]
pub(crate) fn now() -> u64 {
    // SAFETY: rdtsc has no preconditions.
    unsafe { core::arch::x86_64::_rdtsc() }
}

pub(crate) fn note(kind: u64, loc: &'static Location<'static>, bytes: usize, ticks: u64) {
    let _ = START.compare_exchange(0, now(), Relaxed, Relaxed);
    let want = loc as *const _ as *mut Location<'static>;
    for s in &SITES {
        let mut p = s.loc.load(Relaxed);
        if p.is_null() {
            match s.loc.compare_exchange(ptr::null_mut(), want, Relaxed, Relaxed) {
                Ok(_) => {
                    s.kind.store(kind, Relaxed);
                    p = want;
                }
                Err(found) => p = found,
            }
        }
        if p == want && (s.kind.load(Relaxed) == kind || s.kind.load(Relaxed) == u64::MAX) {
            s.calls.fetch_add(1, Relaxed);
            s.bytes.fetch_add(bytes as u64, Relaxed);
            s.ticks.fetch_add(ticks, Relaxed);
            return;
        }
    }
}

pub(crate) fn purged(region: usize) {
    if let Some(w) = LAST_PURGE.get(region) {
        w.store(now(), Relaxed);
    }
}

pub(crate) fn revived(region: usize, bytes: usize, large: bool) {
    if bytes == 0 {
        return;
    }
    let i = large as usize;
    let last = LAST_PURGE.get(region).map_or(0, |w| w.load(Relaxed));
    if last == 0 {
        REVIVED_NEVER[i].fetch_add(bytes as u64, Relaxed);
        return;
    }
    let d = now().saturating_sub(last).max(1);
    let b = 63 - d.leading_zeros() as usize;
    REVIVED[i][b].fetch_add(bytes as u64, Relaxed);
}

struct Buf {
    b: [u8; 512],
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
    let mut b = Buf { b: [0; 512], n: 0 };
    f(&mut b);
    // SAFETY: write(2) from a live local buffer.
    unsafe {
        syscall(nr::WRITE, [int(2), b.b.as_mut_ptr(), int(b.n), int(0), int(0), int(0)]);
    }
}

extern "C" fn dump() {
    let names = ["mprotect", "DONTNEED", "HUGEPAGE", "NOHUGEPAGE", "mincore"];
    let span = now().saturating_sub(START.load(Relaxed));
    emit(|b| {
        let _ = writeln!(b, "DIAG span_ticks {span}");
    });
    for s in &SITES {
        let p = s.loc.load(Relaxed);
        if p.is_null() {
            continue;
        }
        // SAFETY: a &'static Location stored above.
        let loc = unsafe { &*p };
        let file = loc.file();
        let file = file.rsplit_once("src/").map_or(file, |(_, f)| f);
        let kind = s.kind.load(Relaxed) as usize;
        emit(|b| {
            let _ = writeln!(
                b,
                "DIAG site {} {}:{} calls {} bytes {} ticks {}",
                names.get(kind).copied().unwrap_or("?"),
                file,
                loc.line(),
                s.calls.load(Relaxed),
                s.bytes.load(Relaxed),
                s.ticks.load(Relaxed)
            );
        });
    }
    for (i, kind) in ["small", "large"].iter().enumerate() {
        emit(|b| {
            let _ = writeln!(b, "DIAG revived {kind} never_purged_region {}", REVIVED_NEVER[i].load(Relaxed));
        });
        for (k, w) in REVIVED[i].iter().enumerate() {
            let v = w.load(Relaxed);
            if v != 0 {
                emit(|b| {
                    let _ = writeln!(b, "DIAG revived {kind} log2ticks {k} bytes {v}");
                });
            }
        }
    }
}

#[used]
#[unsafe(link_section = ".fini_array")]
static FINI: extern "C" fn() = dump;
