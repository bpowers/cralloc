#!/usr/bin/env python3
"""The jobs under the variants: analyze.py FILE.jsonl... [--calls 'tree depth=20=2,json 32Mi=0.224']

Prints each job's primary metric per allocator, median and the rounds' spread (max - min over the
median), and its phases in the same unit. With --calls, allocator calls per unit of the metric, what
a cycle of hop is worth is the difference chainK - freeK over K x calls: 1 is a job that waits on the
hop like a tight pair and 0 one that does not. Allocators named NAME-padN are pooled as NAME: a mean
over placements, with the spread of the placements' means.
"""
import collections
import json
import re
import statistics as st
import sys

calls = {}
args = sys.argv[1:]
if "--calls" in args:
    i = args.index("--calls")
    for item in args[i + 1].split(","):
        job, value = item.rsplit("=", 1)
        calls[job] = float(value)
    del args[i:i + 2]

rows = collections.defaultdict(lambda: collections.defaultdict(lambda: collections.defaultdict(list)))
units = {}
phases = collections.defaultdict(lambda: collections.defaultdict(lambda: collections.defaultdict(list)))
for path in args:
    for line in open(path):
        r = json.loads(line)
        job, alloc = r.get("job"), r.get("allocator")
        if not job or not alloc or r.get("mode") != "native" or not r.get("valid", True):
            continue
        match = re.fullmatch(r"(.*)-pad(\d+)", alloc)
        name, pad = (match.group(1), match.group(2)) if match else (alloc, "")
        primary = r["primary"]
        rows[job][name][pad].append(float(primary["value"]))
        units[job] = primary["name"]
        workload = r.get("workload") or {}
        metrics = workload.get("metrics") or {}
        denominator = metrics.get("nodes") or metrics.get("bytes") or metrics.get("entries")
        for phase, c in (workload.get("phases") or {}).items():
            if denominator and "cycles" in c:
                phases[job][phase][name, pad].append(c["cycles"] / denominator)

for job, by in rows.items():
    print(f"{job}  ({units[job]})")
    means = {}
    for name in by:
        at = {pad: st.median(v) for pad, v in by[name].items()}
        means[name] = st.mean(at.values())
    base = means.get("cralloc-plain")
    for name in sorted(by):
        at = {pad: st.median(v) for pad, v in by[name].items()}
        spread = (max(at.values()) - min(at.values())) / means[name] * 100 if len(at) > 1 else 0
        rounds = [x for v in by[name].values() for x in v]
        if len(at) == 1:
            spread = (max(rounds) - min(rounds)) / means[name] * 100
        rel = f"{(means[name] / base - 1) * 100:+6.2f}%" if base else ""
        print(f"  {name:<20} {means[name]:>12.4f} {rel}  {len(rounds)} runs over {len(at)} placement(s), spread {spread:.2f}%")
    for phase, per in phases[job].items():
        names = sorted({n for n, _ in per})
        row = f"    {phase:<14}"
        for n in names:
            at = [st.median(v) for (m, _), v in per.items() if m == n]
            row += f" {n.removeprefix('cralloc-')}={st.mean(at):.3f}"
        print(row)
    for name in sorted(by):
        match = re.fullmatch(r"cralloc-chain(\d+)", name)
        free = name.replace("chain", "free")
        if match and free in by:
            k = int(match.group(1))
            d = means[name] - means[free]
            line = f"  chain{k} - free{k} = {d:+.3f}"
            if job in calls:
                line += f", {d / (k * calls[job]):+.3f} of a cycle a call for a cycle of hop"
            print(line)

# Two variants measured at the same placements: the ratio at each, its mean and standard deviation.
variants = sorted({name for by in rows.values() for name in by})
if len(variants) == 2:
    a, b = variants
    print(f"{b} against {a}, percent, by placement:")
    for job, by in rows.items():
        pads = sorted(set(by[a]) & set(by[b]), key=lambda p: int(p or 0))
        ratios = [(st.median(by[b][p]) / st.median(by[a][p]) - 1) * 100 for p in pads]
        if len(ratios) > 1:
            print(f"  {job:<24} mean {st.mean(ratios):+6.2f} sd {st.stdev(ratios):5.2f}  " +
                  " ".join(f"{r:+.2f}" for r in ratios))
