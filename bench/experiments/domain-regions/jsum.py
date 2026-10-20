"""Aggregate benchrun JSONL sessions: per job, each allocator's median of the primary metric,
the median and range of its per-round ratio to the reference (paired within a session's round),
and its median cgroup peak.

usage: python3 -I jsum.py <ref> <alloc,alloc,...> file.jsonl...
"""
import json
import statistics as st
import sys
from collections import defaultdict

ref, allocs, files = sys.argv[1], sys.argv[2].split(","), sys.argv[3:]
val = defaultdict(list)      # (job, alloc) -> values
peak = defaultdict(list)
byround = defaultdict(dict)  # (job, file, round) -> alloc -> value
order = []
for f in files:
    for line in open(f):
        d = json.loads(line)
        if d.get("type") != "run" or not d.get("valid"):
            continue
        job = d["job"] + (" (preload)" if d.get("mode") == "preload" else "")
        a = d["allocator"]
        if job not in order:
            order.append(job)
        v = d["primary"]["value"]
        val[(job, a)].append(v)
        peak[(job, a)].append(d["cgroup"]["peak"] / 2**20)
        byround[(job, f, d["round"])][a] = v

print("| Job | " + " | ".join(allocs) + " | " + " | ".join(f"{a} / {ref}" for a in allocs if a != ref)
      + " | peak " + " / ".join(allocs) + " |")
print("|---|" + "---:|" * (2 * len(allocs) - 1 + 1))
for job in order:
    cells = []
    for a in allocs:
        xs = val[(job, a)]
        cells.append(f"{st.median(xs):.4g}" if xs else "-")
    ratios = []
    for a in allocs:
        if a == ref:
            continue
        rs = [r[a] / r[ref] for k, r in byround.items() if k[0] == job and a in r and ref in r]
        if rs:
            ratios.append(f"{st.median(rs):.3f} ({min(rs):.2f}-{max(rs):.2f})")
        else:
            ratios.append("-")
    pk = " / ".join(f"{st.median(peak[(job, a)]):.1f}" if peak[(job, a)] else "-" for a in allocs)
    n = len(val[(job, ref)])
    print(f"| {job} (n={n}) | " + " | ".join(cells) + " | " + " | ".join(ratios) + f" | {pk} |")
