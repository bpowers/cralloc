import os
import sys
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from sub import sub

# Diagnostic, on top of the diagnostic patch: counts heartbeats and what each step of the
# clocked decay pass decided, and prints them at exit as "DIAG count NAME N".
D = "crates/asmalloc/src/os/linux/diag.rs"
sub(D, "static START: AtomicU64 = AtomicU64::new(0);\n",
    "static START: AtomicU64 = AtomicU64::new(0);\n"
    "pub(crate) static COUNTS: [AtomicU64; 18] = [const { AtomicU64::new(0) }; 18];\n"
    "const COUNT_NAMES: [&str; 18] = [\"beat\", \"decay_step\", \"wait\", \"start\", \"start_empty\",\n"
    "    \"step_purged\", \"decay_bytes\", \"pass_end\", \"region_below_floor\", \"unit_below_floor\",\n"
    "    \"unit_busy\", \"unit_clean\", \"unit_done\", \"tag_mismatch\", \"restamp\", \"first_beat_phase\",\n    \"move_small\", \"move_large\"];\n"
    "#[inline(always)]\n"
    "pub(crate) fn bump(i: usize, n: u64) {\n"
    "    COUNTS[i].fetch_add(n, Relaxed);\n"
    "}\n")
sub(D, "    for (i, kind) in [\"small\", \"large\"].iter().enumerate() {\n",
    "    for (i, c) in COUNTS.iter().enumerate() {\n"
    "        emit(|b| {\n"
    "            let _ = writeln!(b, \"DIAG count {} {}\", COUNT_NAMES[i], c.load(Relaxed));\n"
    "        });\n"
    "    }\n"
    "    for (i, kind) in [\"small\", \"large\"].iter().enumerate() {\n")
H = "crates/asmalloc/src/depot/heartbeat.rs"
sub(H, "    fn beat(&self, served: Option<SmallClass>, chores: Chores, activity: Option<u64>) {\n",
    "    fn beat(&self, served: Option<SmallClass>, chores: Chores, activity: Option<u64>) {\n"
    "        crate::os::linux::diag::bump(0, 1);\n"
    "        // The phase of the clock's interval (in thousandths) at the first heartbeat, plus one.\n"
    "        let _ = crate::os::linux::diag::COUNTS[15].compare_exchange(0, ((crate::os::linux::diag::now() & 0xffff_ffff) * 1000 >> 32) + 1, core::sync::atomic::Ordering::Relaxed, core::sync::atomic::Ordering::Relaxed);\n")
P = "crates/asmalloc/src/back/purge.rs"
sub(P, "        let mut seen = state.decay.word.load(Acquire);\n",
    "        crate::os::linux::diag::bump(1, 1);\n"
    "        let mut seen = state.decay.word.load(Acquire);\n")
sub(P, "            Decay::Wait => return None,\n",
    "            Decay::Wait => {\n                crate::os::linux::diag::bump(2, 1);\n                return None;\n            }\n")
sub(P, "            Decay::Restamp(word) => {\n",
    "            Decay::Restamp(word) => {\n                crate::os::linux::diag::bump(14, 1);\n")
sub(P, "                if retained == 0 {\n",
    "                crate::os::linux::diag::bump(3, 1);\n"
    "                if retained == 0 {\n                    crate::os::linux::diag::bump(4, 1);\n")
sub(P, "            if pass.tag != tag {\n",
    "            if pass.tag != tag {\n                crate::os::linux::diag::bump(13, 1);\n")
sub(P, "                // The goal is met or the walk has gone round. End the pass this thread saw,\n",
    "                crate::os::linux::diag::bump(7, 1);\n"
    "                // The goal is met or the walk has gone round. End the pass this thread saw,\n")
sub(P, "            let done = (purged / page).min(u32::MAX as usize) as u32;\n",
    "            if purged != 0 {\n                crate::os::linux::diag::bump(5, 1);\n                crate::os::linux::diag::bump(6, purged as u64);\n            }\n"
    "            let done = (purged / page).min(u32::MAX as usize) as u32;\n")
sub(P, "            // Everything the cursor has left is below the frontier.\n",
    "            crate::os::linux::diag::bump(8, 1);\n"
    "            // Everything the cursor has left is below the frontier.\n")
sub(P, "                Step::Unit(unit) if unit.start < floor => {\n",
    "                Step::Unit(unit) if unit.start < floor => {\n                    crate::os::linux::diag::bump(9, 1);\n")
sub(P, "                    let (outcome, words) = region::purge_unit(&host, region, unit);\n",
    "                    let (outcome, words) = region::purge_unit(&host, region, unit);\n"
    "                    crate::os::linux::diag::bump(match outcome { Purge::Busy => 10, Purge::Clean => 11, _ => 12 }, 1);\n")
L = "crates/asmalloc/src/heap/large.rs"
sub(L, """        // Growing a large object out of its block: take headroom.""",
    """        crate::os::linux::diag::bump(if small(old).is_some() { 16 } else { 17 }, 1);
        // Growing a large object out of its block: take headroom.""")
M = "crates/asmalloc/src/c/mod.rs"
sub(M, "    let (n, to) = (kept.min(size), NonNull::new(new));\n",
    "    crate::os::linux::diag::bump(if matches!(found, Found::Small(_)) { 16 } else { 17 }, 1);\n"
    "    let (n, to) = (kept.min(size), NonNull::new(new));\n")
