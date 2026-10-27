#!/usr/bin/env python3
"""Turns a scratch copy of the tree into the diagnostic build of docs/research/large-heap-constants.md.
Probe only, never product code:

    python3 -I diag_edit.py TREE

It adds `ASMALLOC_DIAG`, an exported array of counters that watch.py reads from the process's
memory by its symbol, and times, with the cycle counter, the work whose size could grow with the
heap:

- a slow path's heartbeat chores (`Cache::beat`) and, inside them, the back end's purge part
  (`Back::purge_steps_of` for a heartbeat, `Back::note_for_thread` while the thread runs);
- each growth payment, by who paid it: the carve itself, a heartbeat or the thread paying what is
  owed, or a heartbeat paying what the thread left unpaid past its limit;
- the background thread's purge steps, `maintain`'s steps, and its whole rounds;
- each lap of the purge walk (from the end of one lap to the end of the next) and each clocked
  pass (from its start to its end);
- the system calls that change the address space: `mprotect` commits, `MADV_DONTNEED`, the
  huge-page advice and the splits.

It also counts carved bytes, how many growth steps found retained memory over the cap, and what
each pass started with (retained, returnable, goal). The layout of the array is in DIAG below and
in watch.py; the two must agree.
"""
import sys
from pathlib import Path

TREE = Path(sys.argv[1])
SRC = TREE / "crates/asmalloc/src"


def sub(rel, old, new, count=1):
    p = SRC / rel
    s = p.read_text()
    if s.count(old) != count:
        raise SystemExit(f"{rel}: {s.count(old)} matches for {old[:80]!r}")
    p.write_text(s.replace(old, new))


DIAG = r'''//! Scratch diagnostic counters (bench/probes/large-heap-constants/diag_edit.py). Not product
//! code.
#![allow(missing_docs, dead_code, unused_unsafe, clippy::all)]
use core::sync::atomic::{AtomicU64, Ordering::Relaxed};

pub const N: usize = 1024;
pub const KIND_BASE: usize = 64;
pub const KIND_SLOTS: usize = 56;
pub const HIST: usize = 48;

// Timed kinds.
pub const BEAT: usize = 0;
pub const HB_PURGE: usize = 1;
pub const NOTE: usize = 2;
pub const GROW_CARVE: usize = 3;
pub const GROW_OWED: usize = 4;
pub const GROW_LAG: usize = 5;
pub const THREAD_STEP: usize = 6;
pub const MAINT_STEP: usize = 7;
pub const ROUND: usize = 8;
pub const LAP: usize = 9;
pub const PASS: usize = 10;
pub const COMMIT: usize = 11;
pub const DONTNEED: usize = 12;
pub const ADVISE: usize = 13;
pub const SPLIT: usize = 14;

// Scalars.
pub const PASSES: usize = 4;
pub const PASSES_COUNT_ONLY: usize = 5;
pub const PASSES_OVER_CAP: usize = 6;
pub const LAST_RET_SMALL: usize = 7;
pub const LAST_RET_LARGE: usize = 8;
pub const LAST_RETURNABLE: usize = 9;
pub const LAST_GOAL: usize = 10;
pub const SUM_GOAL: usize = 11;
pub const CARVED: usize = 12;
pub const GROWTH_DUE: usize = 13;
pub const GROWTH_DUE_OVER_CAP: usize = 14;
pub const GROWTH_PURGED: usize = 15;
pub const LAPS: usize = 16;
pub const LAPS_OVER_INTERVAL: usize = 17;
pub const LAST_LAP_TICKS: usize = 18;
pub const PASS_START_TSC: usize = 19;
pub const MAX_RETAINED: usize = 20;
pub const LAST_LAP_END_TSC: usize = 21;
pub const DECAY_RETURNED_HB: usize = 22;
pub const DECAY_RETURNED_OTHER: usize = 23;
pub const GROWTH_PAYMENTS_WORK: usize = 24;
pub const ROUNDS: usize = 25;
pub const LAST_LAP_REGIONS: usize = 26;
pub const PASSES_ENDED_ROUND: usize = 27;
pub const PASSES_ENDED_GOAL: usize = 28;
pub const LAST_PASS_PAGES_LEFT: usize = 29;

#[unsafe(no_mangle)]
pub static ASMALLOC_DIAG: [AtomicU64; N] = [const { AtomicU64::new(0) }; N];

#[inline(always)]
pub fn add(i: usize, v: usize) {
    if v != 0 {
        ASMALLOC_DIAG[i].fetch_add(v as u64, Relaxed);
    }
}

#[inline(always)]
pub fn set(i: usize, v: usize) {
    ASMALLOC_DIAG[i].store(v as u64, Relaxed);
}

#[inline(always)]
pub fn get(i: usize) -> u64 {
    ASMALLOC_DIAG[i].load(Relaxed)
}

#[inline(always)]
pub fn max(i: usize, v: u64) {
    ASMALLOC_DIAG[i].fetch_max(v, Relaxed);
}

#[inline(always)]
pub fn now() -> u64 {
    // SAFETY: rdtsc has no preconditions.
    unsafe { core::arch::x86_64::_rdtsc() }
}

/// Records one event of `kind` that took `ticks`, did work if `work`, and moved `bytes`.
#[inline(never)]
pub fn note(kind: usize, ticks: u64, work: bool, bytes: usize) {
    let base = KIND_BASE + kind * KIND_SLOTS;
    ASMALLOC_DIAG[base].fetch_add(1, Relaxed);
    ASMALLOC_DIAG[base + 1].fetch_add(ticks, Relaxed);
    ASMALLOC_DIAG[base + 2].fetch_max(ticks, Relaxed);
    if work {
        ASMALLOC_DIAG[base + 3].fetch_add(1, Relaxed);
    }
    if bytes != 0 {
        ASMALLOC_DIAG[base + 4].fetch_add(bytes as u64, Relaxed);
    }
    let bucket = (63 - ticks.max(1).leading_zeros() as usize).min(HIST - 1);
    ASMALLOC_DIAG[base + 8 + bucket].fetch_add(1, Relaxed);
}

/// Times `f` as one event of `kind`.
#[inline(always)]
pub fn timed<T>(kind: usize, f: impl FnOnce() -> T) -> T {
    let t0 = now();
    let r = f();
    note(kind, now().wrapping_sub(t0), true, 0);
    r
}
'''

(SRC / "diag.rs").write_text(DIAG)
sub("lib.rs", "mod heap;\n", "mod heap;\npub mod diag;\n")

P = "back/purge.rs"
# Carved bytes, and growth steps over the cap.
sub(P, "        let old = self.state.growth.fetch_add(bytes, SeqCst);\n",
    "        crate::diag::add(crate::diag::CARVED, bytes);\n"
    "        let old = self.state.growth.fetch_add(bytes, SeqCst);\n")
sub(P, """        let thread = &self.state.thread;
        let geometry = self.space.plan().geometry();
        if thread.load().duty() == Duty::Slow {
            let _ = self.pay_growth(step);
        } else if old >= growth_unpaid(geometry) {
            let _ = self.pay_growth(step);""",
    """        let thread = &self.state.thread;
        let geometry = self.space.plan().geometry();
        crate::diag::add(crate::diag::GROWTH_DUE, 1);
        if self.retained() > retained_cap(geometry) {
            crate::diag::add(crate::diag::GROWTH_DUE_OVER_CAP, 1);
        }
        if thread.load().duty() == Duty::Slow {
            let _ = self.pay_growth_as(crate::diag::GROW_CARVE, step);
        } else if old >= growth_unpaid(geometry) {
            let _ = self.pay_growth_as(crate::diag::GROW_CARVE, step);""")
# Each payment, timed by who paid it.
sub(P, "    #[cold]\n    #[inline(never)]\n    fn pay_growth(&self, step: usize) -> usize {\n",
    """    #[cold]
    #[inline(never)]
    fn pay_growth_as(&self, kind: usize, step: usize) -> usize {
        let t0 = crate::diag::now();
        let purged = self.pay_growth(step);
        crate::diag::note(kind, crate::diag::now().wrapping_sub(t0), purged != 0, purged);
        crate::diag::add(crate::diag::GROWTH_PURGED, purged);
        if purged != 0 {
            crate::diag::add(crate::diag::GROWTH_PAYMENTS_WORK, 1);
        }
        purged
    }

    #[cold]
    #[inline(never)]
    fn pay_growth(&self, step: usize) -> usize {
""")
sub(P, "        let paid = self.pay_growth(step);\n        (paid, owed())\n",
    "        let paid = self.pay_growth_as(crate::diag::GROW_OWED, step);\n        (paid, owed())\n")
sub(P, """            let step = growth_step(geometry);
            let _ = self.pay_growth(step);
            owing = self.state.growth.load(SeqCst) >= step;""",
    """            let step = growth_step(geometry);
            let _ = self.pay_growth_as(crate::diag::GROW_LAG, step);
            owing = self.state.growth.load(SeqCst) >= step;""")
# The purge part of a heartbeat, and the thread's steps.
sub(P, "    fn purge_steps_of(&self, now: &dyn Fn() -> u64, size: StepSize) -> PassStep {\n",
    """    fn purge_steps_of(&self, now: &dyn Fn() -> u64, size: StepSize) -> PassStep {
        let t0 = crate::diag::now();
        let step = self.purge_steps_of_timed(now, size);
        let kind = if size.heartbeat { crate::diag::HB_PURGE } else { crate::diag::THREAD_STEP };
        crate::diag::note(kind, crate::diag::now().wrapping_sub(t0), step.returned != 0, step.returned);
        step
    }

    fn purge_steps_of_timed(&self, now: &dyn Fn() -> u64, size: StepSize) -> PassStep {
""")
sub(P, "    fn note_for_thread(&self, now: &dyn Fn() -> u64) -> bool {\n",
    """    fn note_for_thread(&self, now: &dyn Fn() -> u64) -> bool {
        let t0 = crate::diag::now();
        let woke = self.note_for_thread_timed(now);
        crate::diag::note(crate::diag::NOTE, crate::diag::now().wrapping_sub(t0), woke, 0);
        woke
    }

    fn note_for_thread_timed(&self, now: &dyn Fn() -> u64) -> bool {
""")
# What the clocked pass returned, a heartbeat's apart.
sub(P, "            spent += read;\n            *returned += purged;\n",
    "            spent += read;\n            *returned += purged;\n"
    "            crate::diag::add(if size.heartbeat { crate::diag::DECAY_RETURNED_HB } else { crate::diag::DECAY_RETURNED_OTHER }, purged);\n")
# Passes: what each started with, and how long each took.
sub(P, """        let pass = Pass {
            tag: Pass::tag_of(word),
            lap: state.walk.laps.load(Relaxed) as u16,""",
    """        crate::diag::add(crate::diag::PASSES, 1);
        if goal == 0 {
            crate::diag::add(crate::diag::PASSES_COUNT_ONLY, 1);
        }
        if retained > retained_cap(geometry) {
            crate::diag::add(crate::diag::PASSES_OVER_CAP, 1);
        }
        crate::diag::set(crate::diag::LAST_RET_SMALL, self.clamped(false));
        crate::diag::set(crate::diag::LAST_RET_LARGE, self.clamped(true));
        crate::diag::set(crate::diag::LAST_RETURNABLE, self.returnable().unwrap_or(usize::MAX));
        crate::diag::set(crate::diag::LAST_GOAL, goal);
        crate::diag::add(crate::diag::SUM_GOAL, goal);
        crate::diag::max(crate::diag::MAX_RETAINED, retained as u64);
        crate::diag::set(crate::diag::PASS_START_TSC, crate::diag::now() as usize);
        let pass = Pass {
            tag: Pass::tag_of(word),
            lap: state.walk.laps.load(Relaxed) as u16,""")
sub(P, """                self.lower_walked();
                let now = self.now();
                if size.heartbeat {
                    self.note_epoch(now);
                }
                let now = now & !ACTIVE;
                let _ = state
                    .decay
                    .word
                    .compare_exchange(seen, now, Release, Relaxed);
                return None;""",
    """                self.lower_walked();
                let now = self.now();
                if size.heartbeat {
                    self.note_epoch(now);
                }
                let now = now & !ACTIVE;
                if state
                    .decay
                    .word
                    .compare_exchange(seen, now, Release, Relaxed)
                    .is_ok()
                {
                    let began = crate::diag::get(crate::diag::PASS_START_TSC);
                    let ticks = crate::diag::now().wrapping_sub(began);
                    let round = pass.pages != 0 || pass.count;
                    crate::diag::note(crate::diag::PASS, ticks, round, 0);
                    crate::diag::add(
                        if round { crate::diag::PASSES_ENDED_ROUND } else { crate::diag::PASSES_ENDED_GOAL },
                        1,
                    );
                    crate::diag::set(crate::diag::LAST_PASS_PAGES_LEFT, pass.pages as usize);
                }
                return None;""")
# Laps of the walk.
sub(P, """            walk.returnable.store(walk.seen.swap(0, Relaxed), Relaxed);
            walk.returned.store(0, Relaxed);
            walk.laps.fetch_add(1, Relaxed);""",
    """            walk.returnable.store(walk.seen.swap(0, Relaxed), Relaxed);
            walk.returned.store(0, Relaxed);
            walk.laps.fetch_add(1, Relaxed);
            let t = crate::diag::now();
            let last = crate::diag::ASMALLOC_DIAG[crate::diag::LAST_LAP_END_TSC].swap(t, Relaxed);
            if last != 0 {
                let ticks = t.wrapping_sub(last);
                crate::diag::note(crate::diag::LAP, ticks, true, 0);
                crate::diag::set(crate::diag::LAST_LAP_TICKS, ticks as usize);
                if ticks > DECAY_TICKS {
                    crate::diag::add(crate::diag::LAPS_OVER_INTERVAL, 1);
                }
            }
            crate::diag::add(crate::diag::LAPS, 1);
            crate::diag::set(crate::diag::LAST_LAP_REGIONS, top);""")

# A slow path's heartbeat chores.
sub("depot/heartbeat.rs",
    "    fn beat(&self, served: Option<SmallClass>, chores: Chores, activity: Option<u64>) {\n",
    """    fn beat(&self, served: Option<SmallClass>, chores: Chores, activity: Option<u64>) {
        let t0 = crate::diag::now();
        self.beat_timed(served, chores, activity);
        crate::diag::note(crate::diag::BEAT, crate::diag::now().wrapping_sub(t0), true, 0);
    }

    fn beat_timed(&self, served: Option<SmallClass>, chores: Chores, activity: Option<u64>) {
""")
# maintain's steps and the thread's rounds.
sub("depot/maintain.rs",
    "    fn maintain_step_of(&self, round: u32, index: u32) -> Stepped {\n",
    """    fn maintain_step_of(&self, round: u32, index: u32) -> Stepped {
        let t0 = crate::diag::now();
        let s = self.maintain_step_of_timed(round, index);
        crate::diag::note(crate::diag::MAINT_STEP, crate::diag::now().wrapping_sub(t0), s.work, 0);
        s
    }

    fn maintain_step_of_timed(&self, round: u32, index: u32) -> Stepped {
""")
sub("depot/thread.rs",
    """        let word = self.source.thread();
        let mut round = None;
        while let Some(step) = self.maintain_step_with_work() {""",
    """        let word = self.source.thread();
        let mut round = None;
        let t0 = crate::diag::now();
        while let Some(step) = self.maintain_step_with_work() {""")
sub("depot/thread.rs",
    """        *returned |= self.thread_steps();
        let round = round.map(|work| work | core::mem::take(returned));""",
    """        *returned |= self.thread_steps();
        crate::diag::note(crate::diag::ROUND, crate::diag::now().wrapping_sub(t0), round.is_some(), 0);
        crate::diag::add(crate::diag::ROUNDS, 1);
        let round = round.map(|work| work | core::mem::take(returned));""")

# System calls that change the address space.
K = "os/linux/kernel.rs"
sub(K, "        unsafe { range_call(nr::MPROTECT, start, len, PROT_READ | PROT_WRITE) }\n",
    "        crate::diag::timed(crate::diag::COMMIT, || unsafe { range_call(nr::MPROTECT, start, len, PROT_READ | PROT_WRITE) })\n")
sub(K, "        unsafe { range_call(nr::MADVISE, start, len, MADV_DONTNEED) }\n",
    "        let t0 = crate::diag::now();\n"
    "        let r = unsafe { range_call(nr::MADVISE, start, len, MADV_DONTNEED) };\n"
    "        crate::diag::note(crate::diag::DONTNEED, crate::diag::now().wrapping_sub(t0), true, len);\n"
    "        r\n")
sub(K, "        unsafe { range_call(nr::MADVISE, start, len, advice) }\n",
    "        crate::diag::timed(crate::diag::ADVISE, || unsafe { range_call(nr::MADVISE, start, len, advice) })\n")
sub(K, "        unsafe { range_call(nr::MADVISE, start, len, MADV_COLD) }\n",
    "        crate::diag::timed(crate::diag::SPLIT, || unsafe { range_call(nr::MADVISE, start, len, MADV_COLD) })\n")
print("large-heap-constants diag edits applied to", TREE)
