"""Summarize mrun.sh logs of `workload shift`, per build: medians over rounds.

usage: python3 -I shiftsum.py <tag>
Columns: cgroup peak MiB; anonymous RSS when A has freed, the most during B, and at B's end;
B's ns per object in its first round (on A's freed memory) and the median of later rounds.
"""
import glob
import os
import re
import statistics as st
import sys
from collections import defaultdict

S = "/tmp/claude-1000/-home-bpowers-src-asmalloc/9350b66e-ff5f-4010-842a-f49ee9af0052/scratchpad/mem"
tag = sys.argv[1]
rows = defaultdict(list)
for path in glob.glob(os.path.join(S, f"{tag}.r*.log")):
    label = os.path.basename(path)[len(tag) + 1:].split(".", 1)[1][: -len(".log")]
    try:
        peak = int(open(path + ".peak").read()) / 2**20
    except (OSError, ValueError):
        continue
    freed = after_max = last = None
    first = []
    later = []
    for line in open(path, errors="replace"):
        if line.startswith("MEM"):
            kv = dict(x.split("=", 1) for x in line.split()[1:])
            rss = int(kv["rss_anon"]) / 2**20
            if kv["phase"] == "freed":
                freed = rss
            elif kv["phase"] == "after":
                after_max = max(after_max or 0, rss)
                last = rss
        elif line.startswith("SHIFT"):
            kv = dict(x.split("=", 1) for x in line.split()[1:])
            (first if kv["round"] == "0" else later).append(float(kv["ns_per_object"]))
    rows[label].append((peak, freed, after_max, last, st.median(first) if first else 0,
                        st.median(later) if later else 0))
print(f"{'build':14} {'n':>2} {'peak':>7} {'freed':>7} {'B max':>7} {'B end':>7} {'ns r0':>7} {'ns later':>8}")
for label in sorted(rows):
    r = rows[label]
    cols = [st.median(x[i] for x in r if x[i] is not None) for i in range(6)]
    print(f"{label:14} {len(r):2} " + " ".join(f"{c:7.1f}" for c in cols))
