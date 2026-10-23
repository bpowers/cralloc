#!/usr/bin/env python3
"""Turns a scratch copy of the repository into a diagnostic build for `pipeline t8`
(docs/research/fast-lookup.md, "pipeline t8"). Never product code: apply it to a copy
(`git archive`), build from the copy, and throw the copy away.

  t8_edit.py REPO [delay]

The allocator gets `prof.rs`: an exported table of counters, one row of 16 per CPU number (from
the TSC_AUX that `rdtscp` reads), incremented on the Rust slow paths and on every depot stack
push and pop, with a count of the compare-and-swaps that failed. The fast paths are not touched.
The workload binary prints the table's column sums to stderr at exit.

With `delay`, the slab word's address in the inlined fast path also goes through a multiply by
a run-time 1 (three cycles of latency on the path to the routine's slab-word load, two more
instructions), to see whether the lookup's latency decides the job.
"""
import sys
from pathlib import Path

REPO = Path(sys.argv[1])
DELAY = len(sys.argv) > 2 and sys.argv[2] == "delay"
SRC = REPO / "crates/asmalloc/src"

PROF = r'''//! Scratch counters (bench/probes/t8/t8_edit.py). Not product code.
#![allow(missing_docs, dead_code)]
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering::Relaxed};

pub const ROW: usize = 16;
pub const ROWS: usize = 64;
pub const SLOW_ALLOC: usize = 0;
pub const SLOW_FREE: usize = 1;
pub const PUSH: usize = 2;
pub const PUSH_RETRY: usize = 3;
pub const POP: usize = 4;
pub const POP_RETRY: usize = 5;
pub const POP_EMPTY: usize = 6;
pub const NAMES: [&str; 7] =
    ["slow_alloc", "slow_free", "push", "push_retry", "pop", "pop_retry", "pop_empty"];

#[repr(C, align(64))]
pub struct Table(pub [AtomicU64; ROW * ROWS]);

#[unsafe(no_mangle)]
pub static ASMALLOC_T8PROF: Table = Table([const { AtomicU64::new(0) }; ROW * ROWS]);

/// One: a run-time value the compiler cannot fold, for the delay variant. Exported, or LLVM
/// sees that nothing stores to it and folds the load.
#[unsafe(no_mangle)]
pub static ASMALLOC_T8_ONE: AtomicUsize = AtomicUsize::new(1);

#[inline(always)]
fn row() -> usize {
    let mut aux = 0u32;
    // SAFETY: rdtscp has no preconditions on x86-64; Linux keeps the CPU number in TSC_AUX.
    let _ = unsafe { core::arch::x86_64::__rdtscp(&mut aux) };
    (aux & 0xfff) as usize % ROWS
}

#[inline(always)]
pub fn one(i: usize) {
    ASMALLOC_T8PROF.0[row() * ROW + i].fetch_add(1, Relaxed);
}

/// The column sums.
pub fn sums() -> [u64; ROW] {
    let mut out = [0; ROW];
    for r in 0..ROWS {
        for (i, o) in out.iter_mut().enumerate() {
            *o += ASMALLOC_T8PROF.0[r * ROW + i].load(Relaxed);
        }
    }
    out
}
'''


def sub(rel, old, new, count=1):
    p = REPO / rel
    s = p.read_text()
    n = s.count(old)
    if n != count:
        raise SystemExit(f"{rel}: {n} matches for {old[:80]!r}")
    p.write_text(s.replace(old, new))


(SRC / "prof.rs").write_text(PROF)
sub("crates/asmalloc/src/lib.rs", "mod heap;\n", "mod heap;\n#[doc(hidden)]\npub mod prof;\n")

sub("crates/asmalloc/src/heap/mod.rs", """    fn rust_alloc_slow(self, class: SmallClass) -> *mut u8 {
""", """    fn rust_alloc_slow(self, class: SmallClass) -> *mut u8 {
        crate::prof::one(crate::prof::SLOW_ALLOC);
""")
sub("crates/asmalloc/src/heap/mod.rs", """    unsafe fn rust_free_slow(self, class: SmallClass, object: NonNull<u8>) {
""", """    unsafe fn rust_free_slow(self, class: SmallClass, object: NonNull<u8>) {
        crate::prof::one(crate::prof::SLOW_FREE);
""")

stack = "crates/asmalloc/src/depot/stack.rs"
sub(stack, """                Ok(_) => {
                    return Ok(Pushed {""", """                Ok(_) => {
                    crate::prof::one(crate::prof::PUSH);
                    return Ok(Pushed {""")
sub(stack, """                Ok(_) => return Some((magazine, contended)),""",
    """                Ok(_) => {
                    crate::prof::one(crate::prof::POP);
                    return Some((magazine, contended));
                }""")
p = REPO / stack
s = p.read_text()
i = s.index("fn pop_noting")
j = s.index("let top = MagazineId::new(head.number)?;", i)
s = s[:j] + """let Some(top) = MagazineId::new(head.number) else {
                crate::prof::one(crate::prof::POP_EMPTY);
                return None;
            };""" + s[j + len("let top = MagazineId::new(head.number)?;"):]
k = s.index("fn pop_noting")
before, after = s[:k], s[k:]
before = before.replace("""                Err(now) => {
                    contended = true;""", """                Err(now) => {
                    crate::prof::one(crate::prof::PUSH_RETRY);
                    contended = true;""", 1)
after = after.replace("""                Err(now) => {
                    contended = true;""", """                Err(now) => {
                    crate::prof::one(crate::prof::POP_RETRY);
                    contended = true;""", 1)
p.write_text(before + after)

if DELAY:
    sub("crates/asmalloc/src/front/mod.rs", """            .wrapping_add(Field::hot(class).offset())
            .addr()""", """            .wrapping_add(Field::hot(class).offset())
            .addr()
            .wrapping_mul(crate::prof::ASMALLOC_T8_ONE.load(core::sync::atomic::Ordering::Relaxed))""")

sub("bench/workloads/src/main.rs", """fn main() {
    bench_alloc::init();""", """fn main() {
    struct Dump;
    impl Drop for Dump {
        fn drop(&mut self) {
            unsafe extern "C" {
                static ASMALLOC_T8PROF: [u64; 16 * 64];
            }
            const NAMES: [&str; 7] =
                ["slow_alloc", "slow_free", "push", "push_retry", "pop", "pop_retry", "pop_empty"];
            let mut sums = [0u64; 16];
            for r in 0..64 {
                for (i, o) in sums.iter_mut().enumerate() {
                    // SAFETY: the allocator's counter table, read once every thread has finished.
                    *o += unsafe { core::ptr::read_volatile(&raw const ASMALLOC_T8PROF[r * 16 + i]) };
                }
            }
            let named: Vec<String> =
                NAMES.iter().zip(sums.iter()).map(|(n, v)| format!("{n}={v}")).collect();
            eprintln!("T8PROF {}", named.join(" "));
        }
    }
    let _dump = Dump;
    bench_alloc::init();""")
