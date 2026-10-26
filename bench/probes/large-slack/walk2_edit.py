#!/usr/bin/env python3
"""Lets a young region's walk by slot ages start twice a hold instead of once, in a scratch
copy of the tree (docs/research/large-slack.md, "Once a hold"). Probe only.

    python3 -I walk2_edit.py TREE
"""
import sys
from pathlib import Path

p = Path(sys.argv[1]) / "crates/asmalloc/src/back/hold.rs"
s = p.read_text()
old = "    walk.cursor != 0 || count.wrapping_sub(walk.began).saturating_add(1) >= steps\n"
if s.count(old) != 1:
    sys.exit(f"walk2_edit: expected one of {old!r}")
p.write_text(s.replace(old, "    walk.cursor != 0 || count.wrapping_sub(walk.began).saturating_add(1) >= steps / 2\n"))
print(f"walk2_edit: applied to {p}")
