#!/usr/bin/env python3
"""Turns an extracted tree of main at 1eea9af into a scratch build whose purge duty is chosen at
run time (docs/research/meili-tail.md). Never product code: build-at.sh applies it to a copy.

  switch_edit.py TREE

With `ASMALLOC_DIAG_SLOW_PURGE=1` in the environment when the background thread is created,
the process behaves as main's parent (2754021) did: slow paths take every purge step whether
the thread runs or not (`Snapshot::duty` always says `Duty::Slow`), and the thread runs only
`maintain`'s rounds and its timed sleep, with no purge steps of its own and no `SLEEPING` mark,
so no slow path wakes it for purge work. With any other value, or unset, it is main. One
binary for both, so the comparison does not depend on where code lands
(docs/research/grpc-placement.md). The variable is read once, as the thread is created; before
that the duty is the slow paths' in either mode.
"""
import sys
from pathlib import Path

SRC = Path(sys.argv[1]) / "crates/asmalloc/src"


def sub(rel, old, new):
    p = SRC / rel
    s = p.read_text()
    if s.count(old) != 1:
        raise SystemExit(f"{rel}: {s.count(old)} matches for {old[:70]!r}")
    p.write_text(s.replace(old, new))


sub(
    "thread_word.rs",
    """    #[inline(always)]
    pub const fn duty(self) -> Duty {
        match self.state() {""",
    """    #[inline(always)]
    pub fn duty(self) -> Duty {
        if SLOW_PURGE.load(core::sync::atomic::Ordering::Relaxed) {
            return Duty::Slow;
        }
        match self.state() {""",
)
sub(
    "thread_word.rs",
    """/// The thread's word.
""",
    """/// Scratch switch (bench/probes/meili-tail/switch_edit.py): the parent's purge duty.
pub static SLOW_PURGE: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// The thread's word.
""",
)
sub(
    "heap/background.rs",
    """    let mut thread = 0usize;
    // SAFETY: BACKGROUND-03.
    unsafe {
        sigfillset(&mut all);""",
    """    let mut thread = 0usize;
    // SAFETY: a scratch build; getenv does not allocate and the string is read in place.
    unsafe {
        let v = getenv(c"ASMALLOC_DIAG_SLOW_PURGE".as_ptr());
        if !v.is_null() && CStr::from_ptr(v).to_bytes() == b"1" {
            crate::thread_word::SLOW_PURGE.store(true, Relaxed);
        }
    }
    // SAFETY: BACKGROUND-03.
    unsafe {
        sigfillset(&mut all);""",
)
sub(
    "depot/thread.rs",
    """    pub fn thread_turn(&self, seen: &mut Seen, key: u64, returned: &mut bool, every_ns: u64) {
        let word = self.source.thread();
""",
    """    pub fn thread_turn(&self, seen: &mut Seen, key: u64, returned: &mut bool, every_ns: u64) {
        let word = self.source.thread();
        if crate::thread_word::SLOW_PURGE.load(core::sync::atomic::Ordering::Relaxed) {
            // The parent's loop (2754021, `heap::Bound::run_thread`).
            let mut round = None;
            while let Some(step) = self.maintain_step_with_work() {
                round = Some(round.unwrap_or(false) | step.work);
                if !step.more {
                    break;
                }
            }
            match self.rest(seen, key, round) {
                Rest::Park => self.os().wait(word.futex(), key | PARKED, None),
                Rest::Sleep => self.os().wait(word.futex(), word.load().0, Some(every_ns)),
            }
            return;
        }
""",
)
print("switch_edit: ASMALLOC_DIAG_SLOW_PURGE=1 selects the parent's purge duty")
