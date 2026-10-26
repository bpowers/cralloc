#!/usr/bin/env python3
"""Puts acct.rs into a scratch copy of the allocator as `crate::acct`, for measurement only:
counts the tail's raises, lowerings, splits and purges, and, for purges of runs whose pages
were all marked clean, how many of those pages the kernel said were resident just before
(`Os::resident`). Totals go to the file `ACCT_DIAG` names at each lap's end. Anchors are
exact text from `huge-tail-accounting` at the accounting by distrust; it fails loudly on a
mismatch.

  acct_edit.py TREE
"""
import shutil
import sys
from pathlib import Path

TREE = Path(sys.argv[1])
HERE = Path(__file__).resolve().parent
SRC = TREE / "crates/asmalloc/src"


def sub(path, old, new, count=1):
    p = SRC / path
    s = p.read_text()
    if s.count(old) != count:
        raise SystemExit(f"{path}: {s.count(old)} matches (want {count}) for {old[:90]!r}")
    p.write_text(s.replace(old, new))


shutil.copy(HERE / "acct.rs", SRC / "acct.rs")
sub("lib.rs", "extern crate std;\n", "extern crate std;\n\npub mod acct;\n")
P = "back/purge.rs"
sub(P, "            walk.laps.fetch_add(1, Relaxed);\n",
    "            let lap = walk.laps.fetch_add(1, Relaxed);\n            crate::acct::dump(u64::from(lap) + 1);\n")
sub(P, "        let (outcome, _) = region::purge_unit_as(host, region, run, trust);\n",
    """        crate::acct::add(&crate::acct::SPLITS, 1);
        if !trust && region::dirty_pages(host, region, run).0 == 0 {
            let len = (run.end - run.start) as usize;
            let at = self.space.region(region).map_addr(|a| a.saturating_add(run.start as usize * page));
            // SAFETY: the run lies in the region's reservation; the call reads only.
            let resident = unsafe { self.space.os().resident(at, len * page) }.unwrap_or(0);
            crate::acct::add(&crate::acct::CLEAN_PURGES, 1);
            crate::acct::add(&crate::acct::CLEAN_PAGES, len as u64);
            crate::acct::add(&crate::acct::CLEAN_RESIDENT, resident as u64);
        }
        crate::acct::add(&crate::acct::PURGES, 1);
        crate::acct::add(&crate::acct::PURGE_PAGES, (run.end - run.start) as u64);
        let (outcome, _) = region::purge_unit_as(host, region, run, trust);
""")
sub(P, "            self.refuse_tail(REFUSED_LOWER);\n        }\n        if self.marks_due() {",
    "            self.refuse_tail(REFUSED_LOWER);\n        }\n        if moved.change == tail::Change::Done {\n            crate::acct::add(&crate::acct::LOWERINGS, 1);\n        }\n        if self.marks_due() {")
sub(P, "            if let Some(under) = moved.under {\n                self.age_raise(&host, region, under, epoch);\n            }",
    "            if let Some(under) = moved.under {\n                crate::acct::add(&crate::acct::RAISES, 1);\n                self.age_raise(&host, region, under, epoch);\n            }")
