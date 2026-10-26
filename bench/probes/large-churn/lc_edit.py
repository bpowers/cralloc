#!/usr/bin/env python3
"""Applies the large-churn probe to a scratch copy of the tree (docs/research/large-churn.md).

    python3 -I lc_edit.py TREE

Copies lc.rs in as crates/asmalloc/src/back/lc.rs, calls it from the large claim (before it
settles) and the large free, exports `Asmalloc::lc_report`, and has bench-alloc's memstat
sampler write the report after each breakdown it is asked for. Probe only: never merged.
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


(crate / "back/lc.rs").write_text(
    (here / "lc.rs").read_text()
    + """
impl<O: Os> Back<'_, O> {
    /// The probe's report (`lc::report`).
    pub fn lc_report(&self, out: &mut dyn FnMut(&str)) {
        report(self, out);
    }
}
"""
)
edit("crates/asmalloc/src/back/mod.rs", "mod layout;\n", "mod layout;\nmod lc;\n")
edit("crates/asmalloc/src/back/region.rs", "fn page_words(from: u32, to: u32)", "pub(super) fn page_words(from: u32, to: u32)")
edit("crates/asmalloc/src/back/region.rs", "fn count(bits: u64) -> usize {", "pub(super) fn count(bits: u64) -> usize {")
edit(
    "crates/asmalloc/src/back/class.rs",
    "                let settled = region::settle_slot(&host, taken.region, slot, size);",
    "                super::lc::claim(self, &host, taken.region, slot, size, region::recorded(&host, taken.region, slot));\n"
    "                let settled = region::settle_slot(&host, taken.region, slot, size);",
)
edit(
    "crates/asmalloc/src/back/class.rs",
    "    fn free_slot(&self, host: &ClassHost<'a, O>, class: Class, region: RegionId, slot: u32) {\n",
    "    fn free_slot(&self, host: &ClassHost<'a, O>, class: Class, region: RegionId, slot: u32) {\n"
    "        super::lc::free(self, host, region, slot);\n",
)
edit(
    "crates/asmalloc/src/heap/global.rs",
    "    /// Walks the heap for `visitor`: every region, free run and cached object.",
    """    /// The large-churn probe's report of slack in occupied large slots.
    pub fn lc_report(out: &mut dyn FnMut(&str)) {
        if let Some(back) = process().heap.back() {
            back.lc_report(out);
        }
    }

    /// Walks the heap for `visitor`: every region, free run and cached object.""",
)
edit(
    "bench/alloc/src/memstat.rs",
    '            Sampler::dump(&mut sampler.out, "acct", &snap);\n',
    '            Sampler::dump(&mut sampler.out, "acct", &snap);\n'
    "            {\n"
    "                let out = &mut sampler.out;\n"
    '                asmalloc::Asmalloc::lc_report(&mut |line| {\n'
    '                    let _ = writeln!(out, "{line}");\n'
    "                });\n"
    "            }\n",
)
print(f"lc_edit: applied to {tree}")
