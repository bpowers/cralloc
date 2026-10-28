#!/usr/bin/env python3
"""Changes constants of the allocator in a scratch copy of the tree, for the variants of
docs/research/trough-gap.md. Probe only, never product code:

    TG_CONST="FILE:NAME=VALUE[,FILE:NAME=VALUE...]" python3 -I const_edit.py TREE

FILE is a path under crates/asmalloc/src; NAME a `pub const` of an integer type in it.
"""
import os
import re
import sys
from pathlib import Path

tree = Path(sys.argv[1])
for item in os.environ["TG_CONST"].split(","):
    rel, _, assign = item.partition(":")
    name, _, value = assign.partition("=")
    p = tree / "crates/asmalloc/src" / rel
    s = p.read_text()
    pat = re.compile(rf"^((?:pub )?const {re.escape(name)}: (?:u8|u32|u64|usize) = )[^;]+;$", re.M)
    if len(pat.findall(s)) != 1:
        sys.exit(f"{rel}: {name} not found once")
    s = pat.sub(lambda m: f"{m.group(1)}{int(value)};", s)
    p.write_text(s)
    print(f"const_edit: {rel} {name} = {value}")
