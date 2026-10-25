#!/usr/bin/env python3
"""A variant of the outgrown-slot rule for docs/research/grown-slots.md ("The rule built"): an
outgrown slot advised for huge pages is not kept but returned behind the copy, as main returns
it. Applied to a scratch tree of the branch by build-plain.sh's EDIT; not product code.

  nohuge_edit.py TREE
"""
import sys
from pathlib import Path

p = Path(sys.argv[1]) / "crates/asmalloc/src/back/class.rs"
s = p.read_text()
old = """                !region::recorded(host, *region, *index).is_some_and(|r| r.unwritten)
                    && self.keep_outgrown(class, region::charge(host, *region, *index))"""
new = """                !region::recorded(host, *region, *index).is_some_and(|r| r.unwritten || r.huge)
                    && self.keep_outgrown(class, region::charge(host, *region, *index))"""
assert s.count(old) == 1
p.write_text(s.replace(old, new))
print("nohuge edit applied to", sys.argv[1])
