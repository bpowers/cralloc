#!/usr/bin/env python3
"""The clocked pass's goal in a scratch copy of the tree (docs/research/trough-gap.md, "How much
a pass returns"). Probe only, never product code:

    TG_DECAY=A|B|C python3 -I decay_edit.py TREE

A: as built: the larger of a quarter of the retained count and what is over the cap, held to
   what the last lap could return less the floor.
B: a quarter of what the last lap could return (at least the decay floor's 2 MiB), down to the
   floor; the cap plays no part.
C: everything the last lap could return, down to the floor.
"""
import os
import sys
from pathlib import Path

tree = Path(sys.argv[1])
arm = os.environ["TG_DECAY"]
p = tree / "crates/asmalloc/src/back/purge.rs"
s = p.read_text()
old = """        let goal = decay_goal(geometry, retained)
            .max(over)
            .min(returnable.saturating_sub(floor));"""
if s.count(old) != 1:
    sys.exit("decay_edit: the goal's expression not found once")
new = {
    "A": old,
    "B": """        let _ = over;
        let goal = decay_goal(geometry, returnable).min(returnable.saturating_sub(floor));""",
    "C": """        let _ = over;
        let goal = returnable.saturating_sub(floor);""",
}[arm]
p.write_text(s.replace(old, new))
print(f"decay_edit: arm {arm}")
