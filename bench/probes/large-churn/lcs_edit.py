#!/usr/bin/env python3
"""Applies the large-churn switch (lcs.rs) to a scratch copy of the tree, for
docs/research/large-churn.md. Probe only: never merged.

    python3 -I lcs_edit.py TREE

Copies lcs.rs in as crates/asmalloc/src/back/lcs.rs and calls it from the large claim (after
it settles), a resize in place, a large free and the old slot of a move, and from the hold's
look at a region whose stamp is young. Exports `Asmalloc::lcs_report`; with bench-alloc's
memstat build, each breakdown asked for also writes the switch's counters.
"""
import sys
from pathlib import Path

tree = Path(sys.argv[1])
here = Path(__file__).resolve().parent
crate = tree / "crates/asmalloc/src"


def edit(rel, old, new, count=1):
    p = tree / rel
    s = p.read_text()
    if s.count(old) != count:
        sys.exit(f"{rel}: expected {count} of {old!r}, found {s.count(old)}")
    p.write_text(s.replace(old, new))


(crate / "back/lcs.rs").write_text((here / "lcs.rs").read_text())
edit("crates/asmalloc/src/back/mod.rs", "mod layout;\n", "mod layout;\npub(crate) mod lcs;\n")
edit("crates/asmalloc/src/back/region.rs", "fn page_words(from: u32, to: u32)", "pub(super) fn page_words(from: u32, to: u32)")
edit("crates/asmalloc/src/back/region.rs", "fn count(bits: u64) -> usize {", "pub(super) fn count(bits: u64) -> usize {")
edit("crates/asmalloc/src/back/hold.rs", "const fn stamp_of(count: u32) -> u64 {", "pub(super) const fn stamp_of(count: u32) -> u64 {")
edit(
    "crates/asmalloc/src/back/class.rs",
    "                let settled = region::settle_slot(&host, taken.region, slot, size);\n",
    "                let settled = region::settle_slot(&host, taken.region, slot, size);\n"
    "                super::lcs::claim(&host, taken.region, slot, size, region::recorded(&host, taken.region, slot));\n",
)
edit(
    "crates/asmalloc/src/back/class.rs",
    "    fn free_slot(&self, host: &ClassHost<'a, O>, class: Class, region: RegionId, slot: u32) {\n",
    "    fn free_slot(&self, host: &ClassHost<'a, O>, class: Class, region: RegionId, slot: u32) {\n"
    "        super::lcs::left(self, host, region, slot);\n",
)
edit(
    "crates/asmalloc/src/back/class.rs",
    "        let Some(class) = slot else {\n            return;\n        };\n        if keep {\n",
    "        let Some(class) = slot else {\n            return;\n        };\n"
    "        super::lcs::left(self, &host, region, index);\n        if keep {\n",
)
edit(
    "crates/asmalloc/src/back/class.rs",
    "                        region::mark_clean(&host, region, slot, from, len);\n"
    "                    }\n                }\n            }\n            Some(Kind::Run(regions)) => {\n",
    "                        region::mark_clean(&host, region, slot, from, len);\n"
    "                    }\n                }\n"
    "                super::lcs::resized(&host, region, slot, old, size);\n"
    "            }\n            Some(Kind::Run(regions)) => {\n",
)
edit(
    "crates/asmalloc/src/back/hold.rs",
    "        if stamp == 0 || !held_for(stamp, count, reach.steps) {\n            return (1, 0, true);\n        }\n",
    "        if stamp != 0 && !held_for(stamp, count, reach.steps) {\n"
    "            // The switch's per-slot ageing (back/lcs.rs): a young region's old free slots.\n"
    "            let (bytes, seen) = self.lcs_young_region(class, region, count, reach.steps, limit, budget);\n"
    "            return (seen.div_ceil(64).max(1), bytes, true);\n"
    "        }\n"
    "        if stamp == 0 {\n            return (1, 0, true);\n        }\n",
)
edit(
    "crates/asmalloc/src/heap/global.rs",
    "    /// Walks the heap for `visitor`: every region, free run and cached object.",
    """    /// The large-churn switch's counters.
    pub fn lcs_report(out: &mut dyn FnMut(&str)) {
        crate::back::lcs::report(out);
    }

    /// Walks the heap for `visitor`: every region, free run and cached object.""",
)
memstat = tree / "bench/alloc/src/memstat.rs"
if "lcs_report" not in memstat.read_text():
    edit(
        "bench/alloc/src/memstat.rs",
        '            Sampler::dump(&mut sampler.out, "acct", &snap);\n',
        '            Sampler::dump(&mut sampler.out, "acct", &snap);\n'
        "            {\n"
        "                let out = &mut sampler.out;\n"
        '                asmalloc::Asmalloc::lcs_report(&mut |line| {\n'
        '                    let _ = writeln!(out, "{line}");\n'
        "                });\n"
        "            }\n",
    )
print(f"lcs_edit: applied to {tree}")
