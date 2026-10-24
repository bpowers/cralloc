#!/usr/bin/env python3
"""The run-time switch for the evidence rule (docs/research/meili-peak.md, "Evidence at the
free"), for measurement only: it replaces the rule's constants with numbers read once from the
environment, so that one binary runs main's behaviour and the rule's. Apply to a scratch copy
of the branch that has the rule (build.sh).

  evidence_edit.py TREE

`ASMALLOC_UNWRITTEN=<bytes>`: the floor of a zeroed request that is marked; 0 or unset marks
nothing, which is main's behaviour. `ASMALLOC_UNWRITTEN_SAMPLES=<n>` (default 16) and
`ASMALLOC_UNWRITTEN_ONE_IN=<k>` (default 8): the sample at the free and its cut.
"""
import sys
from pathlib import Path

TREE = Path(sys.argv[1])
SRC = TREE / "crates/asmalloc/src"


def sub(path, old, new):
    p = SRC / path
    s = p.read_text()
    if s.count(old) != 1:
        raise SystemExit(f"{path}: {s.count(old)} matches for {old[:80]!r}")
    p.write_text(s.replace(old, new))


SWITCH = r'''//! Scratch switch (bench/probes/meili-peak/evidence_edit.py). Not product code.
#![allow(missing_docs, dead_code)]
use core::sync::atomic::{AtomicUsize, Ordering::Relaxed};

unsafe extern "C" {
    fn getenv(name: *const core::ffi::c_char) -> *const core::ffi::c_char;
}

static FLOOR: AtomicUsize = AtomicUsize::new(usize::MAX);
static SAMPLES: AtomicUsize = AtomicUsize::new(usize::MAX);
static ONE_IN: AtomicUsize = AtomicUsize::new(usize::MAX);

fn number(cell: &AtomicUsize, name: &core::ffi::CStr, default: usize) -> usize {
    let seen = cell.load(Relaxed);
    if seen != usize::MAX {
        return seen;
    }
    // SAFETY: getenv returns null or a NUL-terminated string.
    let p = unsafe { getenv(name.as_ptr()) };
    let mut v = default;
    if !p.is_null() {
        v = 0;
        let mut i = 0;
        loop {
            // SAFETY: within the NUL-terminated string, which ends at a byte that is no digit.
            let c = unsafe { *p.add(i) } as u8;
            if !c.is_ascii_digit() {
                break;
            }
            v = v.saturating_mul(10).saturating_add((c - b'0') as usize);
            i += 1;
        }
    }
    cell.store(v, Relaxed);
    v
}

/// Whether a zeroed request of `used` bytes is marked: never when the floor is 0.
pub fn marks(used: usize) -> bool {
    let floor = number(&FLOOR, c"ASMALLOC_UNWRITTEN", 0);
    floor != 0 && used >= floor
}

pub fn samples() -> usize {
    number(&SAMPLES, c"ASMALLOC_UNWRITTEN_SAMPLES", 16).max(1)
}

pub fn one_in() -> usize {
    number(&ONE_IN, c"ASMALLOC_UNWRITTEN_ONE_IN", 8).max(1)
}
'''
(SRC / "switch.rs").write_text(SWITCH)
sub("lib.rs", "mod heap;\n", "mod heap;\npub mod switch;\n")
sub("heap/large.rs", "                    && used >= unwritten_floor(back.space().plan().geometry())\n",
    "                    && crate::switch::marks(used)\n")
sub("heap/large.rs", "use crate::back::policy::{STAGGER_ROOM, STAGGER_STEP, unwritten_floor};",
    "use crate::back::policy::{STAGGER_ROOM, STAGGER_STEP};")
sub("back/class.rs", "        let samples = UNWRITTEN_SAMPLES.min(pages);\n",
    "        let samples = crate::switch::samples().min(pages);\n")
sub("back/class.rs", "        if resident * UNWRITTEN_RESIDENT_ONE_IN > samples {\n",
    "        if resident * crate::switch::one_in() > samples {\n")
print("evidence switch applied to", TREE)
