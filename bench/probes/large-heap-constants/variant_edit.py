#!/usr/bin/env python3
"""Changes one of the allocator's fixed numbers in a scratch copy of the tree, for the
measurement variants of docs/research/large-heap-constants.md. Probe only, never product code:

    LHC_VARIANT=NAME=VALUE python3 -I variant_edit.py TREE

NAME is a constant of crates/asmalloc/src/back/policy.rs counted in huge pages
(RETAINED_CAP_HUGE_PAGES, KEPT_OUTGROWN_HUGE_PAGES, COMMIT_AHEAD_MAX_HUGE_PAGES, ...); VALUE its
new value. Apply after diag_edit.py, so that the variant keeps the counters.
"""
import os
import re
import sys
from pathlib import Path

tree = Path(sys.argv[1])
name, _, value = os.environ["LHC_VARIANT"].partition("=")
p = tree / "crates/asmalloc/src/back/policy.rs"
s = p.read_text()
pat = re.compile(rf"^pub const {re.escape(name)}: usize = \d+;$", re.M)
if len(pat.findall(s)) != 1:
    sys.exit(f"{name}: not found once in policy.rs")
s = pat.sub(f"pub const {name}: usize = {int(value)};", s)
p.write_text(s)
print(f"variant {name} = {value}")
