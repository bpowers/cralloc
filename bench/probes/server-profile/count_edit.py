#!/usr/bin/env python3
"""Turns a scratch copy of the repository into the counting build of
docs/research/server-profile.md. Never product code: apply it to a copy (`git archive`), build
from the copy, and throw the copy away.

  count_edit.py REPO

The allocator (crates/asmalloc) gets `prof.rs`: an exported table `ASMALLOC_PROF` of counters,
one row of 64 per CPU number (from the TSC_AUX that `rdtscp` reads) and a last row for the
background thread, and increments at the slow paths' decisions, the large-object path, resizes
and the system calls the allocator makes. The fast paths are not touched: what they serve is
the calls the wrapper counts less the slow paths counted here.

bench-alloc gets a feature `prof` (src/prof.rs, copied from this directory): asmalloc behind a
wrapper that counts calls and, when a reader sets its state from outside (count.py), records
every call for a window (layout, address, CPU, time, and whether the call took a slow path),
then for a tail one free in 64 by address and every large call, so that lifetimes longer than
the window can be read. The record goes to `$BENCH_PROF_DIR/trace-PID.bin` at exit.

Counter indices are in PROF below and in count.py.
"""
import shutil
import sys
from pathlib import Path

REPO = Path(sys.argv[1])
SRC = REPO / "crates/asmalloc/src"
HERE = Path(__file__).resolve().parent

PROF = r'''//! Scratch counters (bench/probes/server-profile/count_edit.py). Not product code.
#![allow(missing_docs, dead_code)]
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering::Relaxed};

pub const ROW: usize = 64;
pub const ROWS: usize = 65;
/// The background thread's row.
pub const BACKGROUND: usize = 64;

pub const SLOW_ALLOC: usize = 0;
pub const SLOW_FREE: usize = 1;
pub const A_SPARE: usize = 2;
pub const A_DEPOT: usize = 3;
pub const A_FILL: usize = 4;
pub const A_FILL_OBJECTS: usize = 5;
pub const F_SPARE: usize = 6;
pub const F_DEPOT: usize = 7;
pub const F_CREATE: usize = 8;
pub const CENTRE: usize = 9;
pub const TO_SHARED: usize = 10;
pub const LAUNDER: usize = 11;
pub const LAUNDER_OBJECTS: usize = 12;
pub const BEAT: usize = 13;
pub const LARGE_ALLOC: usize = 14;
pub const LARGE_ALLOC_BYTES: usize = 15;
pub const LARGE_FREE: usize = 16;
pub const RESIZE: usize = 17;
pub const RESIZE_SMALL_IN_PLACE: usize = 18;
pub const RESIZE_LARGE_IN_PLACE: usize = 19;
pub const RESIZE_MOVE: usize = 20;
pub const RESIZE_MOVE_BYTES: usize = 21;
pub const SOURCE_ONE: usize = 22;
/// Any slow path, large call or resize that moves: what the wrapper's per-call flag reads.
pub const SLOW_EVENTS: usize = 23;
pub const MPROTECT: usize = 24;
pub const MPROTECT_BYTES: usize = 25;
pub const MADVISE: usize = 26;
pub const MADVISE_BYTES: usize = 27;
pub const MUNMAP: usize = 28;
pub const MEMBARRIER: usize = 29;
pub const FUTEX_WAIT: usize = 30;
pub const FUTEX_WAKE: usize = 31;
pub const MINCORE: usize = 32;
pub const MAINTAIN: usize = 33;
pub const RETIRE_SPARE: usize = 34;
pub const RETIRE_DEPOT: usize = 35;
pub const LARGE_MOVE: usize = 36;
/// Large allocations by log2 of their size, 15 to 34.
pub const LARGE_LOG2: usize = 40;

#[repr(C, align(64))]
pub struct Table(pub [AtomicU64; ROW * ROWS]);

#[unsafe(no_mangle)]
pub static ASMALLOC_PROF: Table = Table([const { AtomicU64::new(0) }; ROW * ROWS]);

/// The background thread's stack, in 2 MiB units, so that its counts go to its own row.
static BACKGROUND_STACK: AtomicUsize = AtomicUsize::new(usize::MAX);

#[inline(always)]
fn stack_unit() -> usize {
    let here = 0u8;
    (&raw const here as usize) >> 21
}

pub fn mark_background() {
    BACKGROUND_STACK.store(stack_unit(), Relaxed);
}

#[inline(always)]
pub fn row() -> usize {
    if stack_unit() == BACKGROUND_STACK.load(Relaxed) {
        return BACKGROUND;
    }
    let mut aux = 0u32;
    // SAFETY: rdtscp has no preconditions on x86-64; Linux keeps the CPU number in TSC_AUX.
    let _ = unsafe { core::arch::x86_64::__rdtscp(&mut aux) };
    (aux & 0xfff) as usize % BACKGROUND
}

#[inline(always)]
pub fn add(i: usize, v: usize) {
    ASMALLOC_PROF.0[row() * ROW + i].fetch_add(v as u64, Relaxed);
}

#[inline(always)]
pub fn one(i: usize) {
    add(i, 1);
}

/// The slow-event count of CPU `cpu`'s row, for the wrapper's per-call flag.
#[inline(always)]
pub fn slow_events(cpu: usize) -> u64 {
    ASMALLOC_PROF.0[(cpu % BACKGROUND) * ROW + SLOW_EVENTS].load(Relaxed)
}
'''


def sub(rel, old, new, count=1):
    p = SRC / rel
    s = p.read_text()
    n = s.count(old)
    if n != count:
        raise SystemExit(f"{rel}: {n} matches for {old[:80]!r}")
    p.write_text(s.replace(old, new))


(SRC / "prof.rs").write_text(PROF)
sub("lib.rs", "mod heap;\n", "mod heap;\n#[doc(hidden)]\npub mod prof;\n")

# Slow paths of small classes.
sub("depot/slow.rs", """    pub fn alloc_slow(&self, class: SmallClass) -> Option<NonNull<u8>> {
        self.begin_slow_path();""", """    pub fn alloc_slow(&self, class: SmallClass) -> Option<NonNull<u8>> {
        crate::prof::one(crate::prof::SLOW_ALLOC);
        crate::prof::one(crate::prof::SLOW_EVENTS);
        self.begin_slow_path();""")
sub("depot/slow.rs", """    pub fn free_slow(&self, class: SmallClass, object: NonNull<u8>) {
        self.begin_slow_path();""", """    pub fn free_slow(&self, class: SmallClass, object: NonNull<u8>) {
        crate::prof::one(crate::prof::SLOW_FREE);
        crate::prof::one(crate::prof::SLOW_EVENTS);
        self.begin_slow_path();""")
sub("depot/slow.rs", """            if magazine.level() > 0 {
                return Obtained::Magazine(magazine);
            }""", """            if magazine.level() > 0 {
                crate::prof::one(crate::prof::A_SPARE);
                return Obtained::Magazine(magazine);
            }""")
sub("depot/slow.rs", """        if let Some(magazine) = self.pop_full(class, &mut chores.tried) {
            return Obtained::Magazine(magazine);
        }""", """        if let Some(magazine) = self.pop_full(class, &mut chores.tried) {
            crate::prof::one(crate::prof::A_DEPOT);
            return Obtained::Magazine(magazine);
        }""")
sub("depot/slow.rs", """        let filled = self.source.fill(class, slots);
        let level = filled.count.min(capacity);""", """        let filled = self.source.fill(class, slots);
        let level = filled.count.min(capacity);
        crate::prof::one(crate::prof::A_FILL);
        crate::prof::add(crate::prof::A_FILL_OBJECTS, level);""")
sub("depot/slow.rs", """            if magazine.level() == 0 {
                return Some(magazine);
            }""", """            if magazine.level() == 0 {
                crate::prof::one(crate::prof::F_SPARE);
                return Some(magazine);
            }""")
sub("depot/slow.rs", """            {
                self.note_return(class, Spare::Empty, contended);
                return Some(magazine);
            }
        }
        self.area.create(size)""", """            {
                self.note_return(class, Spare::Empty, contended);
                crate::prof::one(crate::prof::F_DEPOT);
                return Some(magazine);
            }
        }
        crate::prof::one(crate::prof::F_CREATE);
        self.area.create(size)""")
sub("depot/slow.rs", """        // Both are this thread's now: `old`, empty, out of the slab, and `held`.""",
    """        crate::prof::one(crate::prof::CENTRE);
        // Both are this thread's now: `old`, empty, out of the slab, and `held`.""")
sub("depot/slow.rs", """        // Both are this thread's now: `old`, full, out of the slab, and `held`, empty.""",
    """        crate::prof::one(crate::prof::CENTRE);
        // Both are this thread's now: `old`, full, out of the slab, and `held`, empty.""")

# The depot and launders.
sub("depot/mod.rs", """    fn to_shared(&self, class: SmallClass, magazine: Magazine<'_>) -> bool {
        let level = magazine.level() as u32;""", """    fn to_shared(&self, class: SmallClass, magazine: Magazine<'_>) -> bool {
        crate::prof::one(crate::prof::TO_SHARED);
        let level = magazine.level() as u32;""")
sub("depot/mod.rs", """    fn launder_held(&self, class: SmallClass, magazine: &Magazine<'_>) -> usize {
        let level = magazine.level();""", """    fn launder_held(&self, class: SmallClass, magazine: &Magazine<'_>) -> usize {
        let level = magazine.level();
        crate::prof::one(crate::prof::LAUNDER);
        crate::prof::add(crate::prof::LAUNDER_OBJECTS, level);""")
sub("depot/heartbeat.rs", """    fn beat(&self, served: Option<SmallClass>, chores: Chores, activity: Option<u64>) {
        let clock""", """    fn beat(&self, served: Option<SmallClass>, chores: Chores, activity: Option<u64>) {
        crate::prof::one(crate::prof::BEAT);
        let clock""")

# Large objects and resizes.
sub("heap/large.rs", """        let back = self.start()?;
        let used = used.min(layout.size());""", """        let back = self.start()?;
        crate::prof::one(crate::prof::LARGE_ALLOC);
        crate::prof::one(crate::prof::SLOW_EVENTS);
        crate::prof::add(crate::prof::LARGE_ALLOC_BYTES, layout.size());
        crate::prof::one(crate::prof::LARGE_LOG2 + (usize::BITS - layout.size().leading_zeros()).clamp(15, 34) as usize - 15);
        let used = used.min(layout.size());""")
sub("heap/large.rs", """    pub(crate) unsafe fn free_large(&self, ptr: NonNull<u8>) {
        match self.back() {""", """    pub(crate) unsafe fn free_large(&self, ptr: NonNull<u8>) {
        crate::prof::one(crate::prof::LARGE_FREE);
        crate::prof::one(crate::prof::SLOW_EVENTS);
        match self.back() {""")
sub("heap/large.rs", """        let heap = self.heap();
        let len = heap.block_len(ptr, old);""", """        let heap = self.heap();
        crate::prof::one(crate::prof::RESIZE);
        let len = heap.block_len(ptr, old);""")
sub("heap/large.rs", """        if in_place {
            if small(old).is_some() {""", """        if in_place {
            crate::prof::one(if small(old).is_some() {
                crate::prof::RESIZE_SMALL_IN_PLACE
            } else {
                crate::prof::RESIZE_LARGE_IN_PLACE
            });
            if small(old).is_some() {""")
sub("heap/large.rs", """        let kept = old.size().min(new.size());""", """        let kept = old.size().min(new.size());
        crate::prof::one(crate::prof::RESIZE_MOVE);
        crate::prof::one(crate::prof::SLOW_EVENTS);
        crate::prof::add(crate::prof::RESIZE_MOVE_BYTES, kept);
        if small(old).is_none() {
            crate::prof::one(crate::prof::LARGE_MOVE);
        }""")

# The background thread's row, and its rounds.
sub("heap/mod.rs", """    pub(crate) fn run_thread(self, every_ns: u64) -> ! {
        let word = self.word();""", """    pub(crate) fn run_thread(self, every_ns: u64) -> ! {
        crate::prof::mark_background();
        let word = self.word();""")
sub("heap/mod.rs", """    pub(crate) fn maintain(self) {
        if let Some(back) = self.heap.started() {""", """    pub(crate) fn maintain(self) {
        crate::prof::one(crate::prof::MAINTAIN);
        if let Some(back) = self.heap.started() {""")

# System calls.
sub("os/linux/kernel.rs", """    // SAFETY: KERNEL-01. passed through from the caller. The calls read and write no user memory through""",
    """    match number {
        nr::MPROTECT => {
            crate::prof::one(crate::prof::MPROTECT);
            crate::prof::add(crate::prof::MPROTECT_BYTES, len);
        }
        nr::MADVISE => {
            crate::prof::one(crate::prof::MADVISE);
            crate::prof::add(crate::prof::MADVISE_BYTES, len);
        }
        nr::MUNMAP => crate::prof::one(crate::prof::MUNMAP),
        _ => {}
    }
    // SAFETY: KERNEL-01. passed through from the caller. The calls read and write no user memory through""")
sub("os/linux/kernel.rs", """fn membarrier(command: usize, flags: usize, cpu: u32) -> Result<(), OsError> {
""", """fn membarrier(command: usize, flags: usize, cpu: u32) -> Result<(), OsError> {
    crate::prof::one(crate::prof::MEMBARRIER);
""")
sub("os/linux/kernel.rs", """        let timeout = timeout.as_ref().map_or(ptr::null(), ptr::from_ref);""",
    """        let timeout = timeout.as_ref().map_or(ptr::null(), ptr::from_ref);
        crate::prof::one(crate::prof::FUTEX_WAIT);""")
sub("os/linux/kernel.rs", """    fn wake(&self, word: &AtomicU64) {
""", """    fn wake(&self, word: &AtomicU64) {
        crate::prof::one(crate::prof::FUTEX_WAKE);
""")
sub("os/linux/kernel.rs", """            let at = start.map_addr(|a| a.saturating_add(done));""",
    """            let at = start.map_addr(|a| a.saturating_add(done));
            crate::prof::one(crate::prof::MINCORE);""")

# bench-alloc's wrapper.
BA = REPO / "bench/alloc"
shutil.copy(HERE / "prof.rs", BA / "src/prof.rs")
p = BA / "Cargo.toml"
s = p.read_text()
old = 'live-bytes = ["asmalloc"]\n'
assert s.count(old) == 1
p.write_text(s.replace(old, old + '# Scratch (bench/probes/server-profile): counts and a window of calls.\nprof = ["asmalloc"]\n'))
p = BA / "src/lib.rs"
s = p.read_text()
old = '#[cfg(all(feature = "asmalloc", not(feature = "asmalloc-memstat"), not(feature = "live-bytes")))]'
assert s.count(old) == 1
s = s.replace(old, '#[cfg(all(feature = "asmalloc", not(feature = "asmalloc-memstat"), not(feature = "live-bytes"), '
                   'not(feature = "prof")))]')
old = '#[cfg(feature = "live-bytes")]\nmod live;'
assert s.count(old) == 1
s = s.replace(old, '#[cfg(feature = "prof")]\nmod prof;\n#[cfg(feature = "prof")]\n#[global_allocator]\n'
                   'static GLOBAL: prof::Prof = prof::Prof;\n' + old)
p.write_text(s)
p = BA / "build.rs"
s = p.read_text()
old = '"live-bytes", "memacct"]'
assert s.count(old) == 1
p.write_text(s.replace(old, '"live-bytes", "memacct", "prof"]'))
for app in ("orders", "meilisearch"):
    p = REPO / "bench/apps" / app / "Cargo.toml"
    s = p.read_text()
    old = 'live-bytes = ["bench-alloc/live-bytes"]\n'
    assert s.count(old) == 1, app
    p.write_text(s.replace(old, old + 'prof = ["bench-alloc/prof"]\n'))
print("edited", REPO)
