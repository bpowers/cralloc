#!/usr/bin/env python3
"""Combines `benchrun layout --layouts pads` sessions (one round each is fine): for each job,
each allocator's median per pad over the sessions, the mean and range over the pads, and every
allocator against the first named, by pad00 alone (what a session of single builds compares)
and by the means over the pads.

usage: padsweep.py BASE RESULTS...   (BASE: the allocator to compare the others with)
"""
import collections, json, statistics, sys

base, files = sys.argv[1], sys.argv[2:]
runs = collections.defaultdict(list)
metric = {}
for f in files:
    for line in open(f):
        r = json.loads(line)
        if r.get("type") != "run" or r.get("valid") is not True:
            continue
        value = r.get("primary", {}).get("value")
        name = r.get("allocator", "")
        if value is None or "@" not in name:
            continue
        alloc, layout = name.split("@", 1)
        runs[(r["job"], alloc, layout)].append(value)
        metric[r["job"]] = r["primary"].get("name", "")
jobs = sorted({j for j, _, _ in runs})
for job in jobs:
    allocs = sorted({a for j, a, _ in runs if j == job}, key=lambda a: (a != base, a))
    pads = sorted({l for j, _, l in runs if j == job})
    print(f"\n{job} ({metric[job]}), medians per pad, runs per cell in brackets")
    print(f"{'allocator':18}" + "".join(f"{p:>14}" for p in pads) + f"{'mean':>12}{'range':>10}")
    means, firsts = {}, {}
    for a in allocs:
        meds = {}
        cells = []
        for p in pads:
            v = runs.get((job, a, p), [])
            if v:
                meds[p] = statistics.median(v)
                cells.append(f"{meds[p]:>10.4g} ({len(v)})")
            else:
                cells.append(f"{'-':>14}")
        vals = list(meds.values())
        means[a] = sum(vals) / len(vals)
        firsts[a] = meds.get(pads[0])
        rng = (max(vals) / min(vals) - 1) * 100
        print(f"{a:18}" + "".join(cells) + f"{means[a]:>12.4g}{rng:>9.1f}%")
    if base in means:
        for a in allocs:
            if a == base:
                continue
            f = (f"{(firsts[a] / firsts[base] - 1) * 100:+.1f}%" if firsts[a] and firsts[base] else "-")
            print(f"  {a} against {base}: {pads[0]} {f}, mean over pads {(means[a] / means[base] - 1) * 100:+.1f}%")
