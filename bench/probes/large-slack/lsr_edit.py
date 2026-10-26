#!/usr/bin/env python3
"""Applies the large-slack switch and counters (lsr.rs) to a scratch copy of the tree, for
docs/research/large-slack.md. Probe only: never merged.

    python3 -I lsr_edit.py TREE

Copies lsr.rs in as crates/asmalloc/src/back/lsr.rs; puts the trim at the claim and the hold's
walk of young regions by slot ages behind its switch (ASMALLOC_LSR, both on when unset); times
and counts each trim; and has a large slot's free, and the move out of it, count how much of
what its claim trimmed the tenant faulted back.
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


(crate / "back/lsr.rs").write_text((here / "lsr.rs").read_text())
edit("crates/asmalloc/src/back/mod.rs", "mod layout;\n", "mod layout;\nmod lsr;\n")
edit(
    "crates/asmalloc/src/back/class.rs",
    "        if let Some(slack) = slack {\n            self.trim_slack(&host, region, slot, slack);\n",
    "        if let Some(slack) = slack.and_then(|s| super::lsr::bound(s, size, host.layout().page_size())) {\n            self.trim_slack(&host, region, slot, slack);\n",
)
edit(
    "crates/asmalloc/src/back/class.rs",
    "        if unsafe { region::purge_part(host, region, slot, from, len) } {\n"
    "            region::mark_clean(host, region, slot, from, len);\n"
    "        }\n",
    "        let t0 = host.space.os().ticks();\n"
    "        let done = unsafe { region::purge_part(host, region, slot, from, len) };\n"
    "        super::lsr::trimmed(region, slot, slack, host.space.os().ticks().wrapping_sub(t0), done);\n"
    "        if done {\n"
    "            region::mark_clean(host, region, slot, from, len);\n"
    "        }\n",
)
edit(
    "crates/asmalloc/src/back/class.rs",
    "    fn free_slot(&self, host: &ClassHost<'a, O>, class: Class, region: RegionId, slot: u32) {\n",
    "    fn free_slot(&self, host: &ClassHost<'a, O>, class: Class, region: RegionId, slot: u32) {\n"
    "        super::lsr::departing(host, region, slot);\n",
)
edit(
    "crates/asmalloc/src/back/class.rs",
    "                let (region, index) = self.validate(&host, from.as_ptr(), class);\n",
    "                let (region, index) = self.validate(&host, from.as_ptr(), class);\n"
    "                super::lsr::departing(&host, region, index);\n",
)
edit(
    "crates/asmalloc/src/back/hold.rs",
    "            if self.purgeable(region).is_none() {\n                return passed;\n            }\n",
    "            if !super::lsr::age() || self.purgeable(region).is_none() {\n                return passed;\n            }\n",
)
print(f"lsr_edit: applied to {tree}")
