#!/usr/bin/env python3
"""How many bytes of one section differ between two binaries, and where (first few runs).

  bytediff.py A B [SECTION]   (default .text)
"""
import sys
sys.path.insert(0, __import__("os").path.dirname(__file__))
from elfinfo import sections

name = sys.argv[3] if len(sys.argv) > 3 else ".text"
da, db = open(sys.argv[1], "rb").read(), open(sys.argv[2], "rb").read()
sa, sb = sections(da)[name], sections(db)[name]
a, b = da[sa[1]:sa[1] + sa[2]], db[sb[1]:sb[1] + sb[2]]
diff = [i for i in range(min(len(a), len(b))) if a[i] != b[i]]
print(f"{name}: {len(a)} / {len(b)} bytes, {len(diff)} differ")
runs = []
for i in diff:
    if runs and i <= runs[-1][1] + 8:
        runs[-1][1] = i
    else:
        runs.append([i, i])
print(f"{len(runs)} runs; first: " + ", ".join(f"{sa[0] + s:#x}+{e - s + 1}" for s, e in runs[:12]))
