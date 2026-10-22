#!/usr/bin/env python3
"""Turns a checkout's allocator into a scratch diagnostic build for the burst after a silence
(docs/research/default-thread-servers.md, "The burst after a silence"). Never product code:
apply it, build the diag binaries, and restore the sources (`git checkout crates/`).

  diag_edit.py REPO

It adds `ASMALLOC_DIAG`, an exported array of counters that a reader takes from the process's
memory by its symbol (watch.py), and two switches read from `ASMALLOC_DIAG`, a two-letter
value: a `-` first turns off `maintain`'s drains and sheds, a `-` second turns off its
purges (the hold's and the clocked pass's), and a `-` third turns off every purge a
heartbeat takes on a slow path (the clocked pass's steps, the hold's looks, growth's
payments). `dp` and `dph` change nothing. Counter indices are in
`diag.rs` below and in watch.py.
"""
import sys
from pathlib import Path

REPO = Path(sys.argv[1])
SRC = REPO / "crates/asmalloc/src"


def sub(rel, old, new):
    p = SRC / rel
    s = p.read_text()
    if s.count(old) != 1:
        raise SystemExit(f"{rel}: {s.count(old)} matches for {old[:70]!r}")
    p.write_text(s.replace(old, new))


DIAG = r'''//! Scratch diagnostic counters (bench/probes/default-thread/diag_edit.py). Not product code.
#![allow(missing_docs, dead_code)]
use core::sync::atomic::{AtomicU64, Ordering::Relaxed};

pub const N: usize = 512;
/// Pages a claim found clean (returned to the OS), by class index: the pages the claim's
/// writes fault back.
pub const CLEANED: usize = 0;
/// Bytes a purge returned, by class index, whatever asked for it.
pub const RETURNED: usize = 128;
/// Claims from the bitmaps (fills of magazines and large allocations), by class index.
pub const CLAIMS: usize = 256;
pub const SLOW_ALLOC: usize = 384;
pub const SLOW_FREE: usize = 385;
/// Bytes the clocked pass returned, in passes of long runs and of runs of any length.
pub const DECAY_IDLE: usize = 390;
pub const DECAY_ANY: usize = 391;
/// The part of those `maintain` returned.
pub const DECAY_MAINT: usize = 392;
pub const GROWTH: usize = 393;
/// Bytes the hold returned, and the part `maintain` returned.
pub const HOLD: usize = 394;
pub const HOLD_MAINT: usize = 395;
/// Looks at a due pass, of them those that found retained memory at or under the floor; and
/// what the last look read: retained small, retained large, the floor, the goal it set.
pub const PASS_LOOKS: usize = 400;
pub const PASS_AT_FLOOR: usize = 401;
pub const RET_SMALL: usize = 402;
pub const RET_LARGE: usize = 403;
pub const FLOOR: usize = 404;
pub const GOAL: usize = 405;
pub const GOAL_SUM: usize = 406;
/// `maintain`'s idle drains and sheds that did work.
pub const DRAINS: usize = 410;
pub const SHEDS: usize = 411;
/// Bitmap words read and steps taken by the clocked pass, heartbeats' and `maintain`'s apart,
/// and words read in a silence's scope and otherwise.
pub const HB_WORDS: usize = 420;
pub const MAINT_WORDS: usize = 421;
pub const HB_STEPS: usize = 422;
pub const MAINT_STEPS: usize = 423;
pub const QUIET_WORDS: usize = 424;
pub const LOUD_WORDS: usize = 425;

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

unsafe extern "C" {
    fn getenv(name: *const core::ffi::c_char) -> *const core::ffi::c_char;
}

static MODE: AtomicU64 = AtomicU64::new(0);

fn mode() -> u64 {
    let m = MODE.load(Relaxed);
    if m != 0 {
        return m;
    }
    let mut v = 1u64 << 63;
    // SAFETY: getenv returns null or a NUL-terminated string.
    let p = unsafe { getenv(c"ASMALLOC_DIAG".as_ptr()) };
    if !p.is_null() {
        // SAFETY: as above; the second byte is read only if the first is not the NUL.
        let (a, b) = unsafe { (*p as u8, if *p != 0 { *p.add(1) as u8 } else { 0 }) };
        // SAFETY: as above, the third byte only if the second is not the NUL.
        let c = if b != 0 { unsafe { *p.add(2) as u8 } } else { 0 };
        if c == b'-' {
            v |= 4;
        }
        if a == b'-' {
            v |= 1;
        }
        if b == b'-' {
            v |= 2;
        }
    }
    MODE.store(v, Relaxed);
    v
}

pub fn nodrain() -> bool {
    mode() & 1 != 0
}

pub fn nopurge() -> bool {
    mode() & 2 != 0
}

pub fn nohbpurge() -> bool {
    mode() & 4 != 0
}
'''

(SRC / "diag.rs").write_text(DIAG)
sub("lib.rs", "mod heap;\n", "mod heap;\npub mod diag;\n")

# Pages a claim found clean.
sub("back/region.rs", "    pub(crate) revived: usize,\n}", "    pub(crate) revived: usize,\n    pub(crate) cleaned: usize,\n}")
sub("back/region.rs", "        revived: (cleaned * page).saturating_sub(claimed),\n",
    "        revived: (cleaned * page).saturating_sub(claimed),\n        cleaned,\n")
sub("back/region.rs", """            resident: layout.size(),
            revived: 0,
        };""", """            resident: layout.size(),
            revived: 0,
            cleaned: 0,
        };""")
sub("back/region.rs", """        resident: layout.size() - clean * layout.page_size(),
        revived: 0,
    }""", """        resident: layout.size() - clean * layout.page_size(),
        revived: 0,
        cleaned: clean,
    }""")
sub("back/class.rs", """            let settled = region::settle(&host, taken.region, taken.word, taken.bits);
            resident += settled.resident;""", """            let settled = region::settle(&host, taken.region, taken.word, taken.bits);
            crate::diag::add(crate::diag::CLEANED + class.class().index() as usize, settled.cleaned);
            crate::diag::add(crate::diag::CLAIMS + class.class().index() as usize, 1);
            resident += settled.resident;""")
sub("back/class.rs", "            self.unretain(class, settled.resident);\n",
    "            crate::diag::add(crate::diag::CLEANED + class.index() as usize, settled.cleaned);\n"
    "            crate::diag::add(crate::diag::CLAIMS + class.index() as usize, 1);\n"
    "            self.unretain(class, settled.resident);\n")

# Bytes returned, by class and by rule.
sub("back/purge.rs", "    pub(super) fn purge_region(\n        &self,\n",
    """    pub(super) fn purge_region(
        &self,
        class: Class,
        region: RegionId,
        budget: usize,
        limit: usize,
        resume: bool,
        scope: Scope,
        counting: bool,
    ) -> (usize, usize, bool, usize) {
        let r = self.purge_region_inner(class, region, budget, limit, resume, scope, counting);
        crate::diag::add(crate::diag::RETURNED + class.index() as usize, r.0);
        r
    }

    fn purge_region_inner(
        &self,
""")
sub("back/purge.rs", "            *returned += purged;\n",
    "            *returned += purged;\n"
    "            crate::diag::add(if pass.any { crate::diag::DECAY_ANY } else { crate::diag::DECAY_IDLE }, purged);\n"
    "            if size.words == PURGE_MAINTAIN_STEP_WORDS {\n"
    "                crate::diag::add(crate::diag::DECAY_MAINT, purged);\n"
    "            }\n")
sub("back/purge.rs", "            finished += usize::from(done);\n        }\n",
    "            finished += usize::from(done);\n        }\n        crate::diag::add(crate::diag::GROWTH, purged);\n")
sub("back/purge.rs", "                let retained = self.retained();\n                let floor = self.floor();\n",
    "                let retained = self.retained();\n                let floor = self.floor();\n"
    "                crate::diag::add(crate::diag::PASS_LOOKS, 1);\n"
    "                crate::diag::set(crate::diag::RET_SMALL, self.clamped(false));\n"
    "                crate::diag::set(crate::diag::RET_LARGE, self.clamped(true));\n"
    "                crate::diag::set(crate::diag::FLOOR, floor);\n"
    "                if retained <= floor {\n"
    "                    crate::diag::add(crate::diag::PASS_AT_FLOOR, 1);\n"
    "                }\n")
sub("back/purge.rs", "                // One pass in `PURGE_ANY_RUN_EVERY` takes runs of any length",
    "                crate::diag::set(crate::diag::GOAL, goal);\n"
    "                crate::diag::add(crate::diag::GOAL_SUM, goal);\n"
    "                // One pass in `PURGE_ANY_RUN_EVERY` takes runs of any length")
sub("back/purge.rs", "    pub fn heartbeat_with(&self, now: &dyn Fn() -> u64) {\n",
    "    pub fn heartbeat_with(&self, now: &dyn Fn() -> u64) {\n        if crate::diag::nohbpurge() {\n            return;\n        }\n")
# Bitmap words the clocked pass's steps read: heartbeats' and maintain's apart, and steps.
sub("back/purge.rs", "            spent += read;\n            *returned += purged;\n",
    "            spent += read;\n            *returned += purged;\n"
    "            crate::diag::add(if size.heartbeat { crate::diag::HB_WORDS } else { crate::diag::MAINT_WORDS }, read);\n"
    "            crate::diag::add(if size.heartbeat { crate::diag::HB_STEPS } else { crate::diag::MAINT_STEPS }, 1);\n"
    "            crate::diag::add(if quiet { crate::diag::QUIET_WORDS } else { crate::diag::LOUD_WORDS }, read);\n")
sub("back/hold.rs", "        let step = self.hold_look(count, reach(self));\n",
    "        let step = self.hold_look(count, reach(self));\n        crate::diag::add(crate::diag::HOLD, step.returned);\n")
sub("heap/source.rs", "        let hold = self.hold_maintain_step(now);\n",
    "        let hold = self.hold_maintain_step(now);\n        crate::diag::add(crate::diag::HOLD_MAINT, hold.returned);\n")

# Slow paths.
sub("depot/slow.rs", "    pub fn alloc_slow(&self, class: SmallClass) -> Option<NonNull<u8>> {\n        self.begin_slow_path();\n",
    "    pub fn alloc_slow(&self, class: SmallClass) -> Option<NonNull<u8>> {\n        crate::diag::add(crate::diag::SLOW_ALLOC, 1);\n        self.begin_slow_path();\n")
sub("depot/slow.rs", "    pub fn free_slow(&self, class: SmallClass, object: NonNull<u8>) {\n        self.begin_slow_path();\n",
    "    pub fn free_slow(&self, class: SmallClass, object: NonNull<u8>) {\n        crate::diag::add(crate::diag::SLOW_FREE, 1);\n        self.begin_slow_path();\n")

# maintain: counters and switches.
sub("depot/maintain.rs", "            if generation::drains_enabled(self.depot)\n                && index < cpus\n",
    "            if generation::drains_enabled(self.depot)\n                && !crate::diag::nodrain()\n                && index < cpus\n")
sub("depot/maintain.rs", "                work = self.idle_look(cpu, clock.now(), Disposal::Launder);\n",
    "                work = self.idle_look(cpu, clock.now(), Disposal::Launder);\n"
    "                crate::diag::add(crate::diag::DRAINS, usize::from(work));\n")
sub("depot/maintain.rs", "self.tuning.shed_unused && self.shed_one();\n",
    "self.tuning.shed_unused && !crate::diag::nodrain() && self.shed_one();\n"
    "            crate::diag::add(crate::diag::SHEDS, usize::from(shed));\n")
sub("depot/maintain.rs", "        let purged = self.source.purge_pass_step(&|| clock.now());\n",
    "        let purged = if crate::diag::nopurge() {\n"
    "            super::source::PurgeStep::default()\n"
    "        } else {\n"
    "            self.source.purge_pass_step(&|| clock.now())\n"
    "        };\n")
print("diag edits applied to", REPO)

# The memstat sampler (bench-alloc's asmalloc-memstat): a full breakdown at every sample with
# ASMALLOC_MEMSTAT_EACH=1, and the process ID appended to the file's name, so that a session's
# runs each keep their own.
M = REPO / "bench/alloc/src/memstat.rs"


def subm(old, new):
    s = M.read_text()
    if s.count(old) != 1:
        raise SystemExit(f"memstat.rs: {s.count(old)} matches for {old[:70]!r}")
    M.write_text(s.replace(old, new))


subm("    start_ms: u64,\n    interval_ms: u64,\n}", "    start_ms: u64,\n    interval_ms: u64,\n    each: bool,\n}")
subm("        if u[0] + snap.runs_res > self.peak.uses()[0] + self.peak.runs_res || self.peak.heap() == 0 {\n            self.peak = *snap;\n        }\n",
     "        if u[0] + snap.runs_res > self.peak.uses()[0] + self.peak.runs_res || self.peak.heap() == 0 {\n            self.peak = *snap;\n        }\n"
     "        if self.each {\n            let snap = self.walk.snap;\n            Sampler::dump(&mut self.out, \"at\", &snap);\n        }\n")
subm("        s.interval_ms = env_num(c\"ASMALLOC_MEMSTAT_MS\", 10).max(1);\n",
     "        s.interval_ms = env_num(c\"ASMALLOC_MEMSTAT_MS\", 10).max(1);\n        s.each = env_num(c\"ASMALLOC_MEMSTAT_EACH\", 0) != 0;\n")
subm("    let fd = unsafe { libc::open(path, libc::O_WRONLY | libc::O_CREAT | libc::O_TRUNC | libc::O_CLOEXEC, 0o644) };\n",
     "    let mut named = [0u8; 600];\n"
     "    // SAFETY: `path` is a NUL-terminated string.\n"
     "    let given = unsafe { std::ffi::CStr::from_ptr(path) }.to_bytes();\n"
     "    let suffix = format!(\".{}\\0\", std::process::id());\n"
     "    let n = given.len().min(named.len() - suffix.len());\n"
     "    named[..n].copy_from_slice(&given[..n]);\n"
     "    named[n..n + suffix.len()].copy_from_slice(suffix.as_bytes());\n"
     "    // SAFETY: `named` is NUL-terminated.\n"
     "    let fd = unsafe { libc::open(named.as_ptr().cast(), libc::O_WRONLY | libc::O_CREAT | libc::O_TRUNC | libc::O_CLOEXEC, 0o644) };\n")
print("memstat edits applied")
