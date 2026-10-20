"""Sum XCOUNT lines over domains for each run, then median per lib, in millions.

usage: python3 -I xsum.py <out dir> <tag>
"""
import glob
import os
import statistics as st
import sys
from collections import defaultdict

out, tag = sys.argv[1], sys.argv[2]
per = defaultdict(list)
for path in sorted(glob.glob(os.path.join(out, f"{tag}.*.err"))):
    lib = os.path.basename(path).split(".")[1]
    total = defaultdict(int)
    for line in open(path, errors="replace"):
        if line.startswith("XCOUNT"):
            for kv in line.split()[2:]:
                k, v = kv.split("=")
                total[k] += int(v)
    if total:
        per[lib].append(total)
keys = ["fill_own", "fill_other", "fill_carved", "pop_shared", "pop_shared_other",
        "pop_shared_other_objects", "pop_empty_shared_other", "laundered", "own_first_objects"]
print(f"{'lib':14}" + "".join(f"{k[:16]:>17}" for k in keys))
for lib, rs in per.items():
    row = []
    for k in keys:
        v = st.median(r[k] for r in rs)
        row.append(f"{v / 1e6:17.2f}" if "objects" in k or k.startswith(("fill", "laund"))
                   else f"{v / 1e3:16.1f}k")
    print(f"{lib:14}" + "".join(row))
