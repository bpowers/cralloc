#!/usr/bin/env python3
"""The others against cralloc on the kernel suite, by group, as standings.md tabulates them:
groups.py [--base NAME] [--allocators a,b,...] FILE...

Each job's cost is the median over its valid runs (time, or the inverse of a rate). A cell is the
geometric mean over the group's jobs of the allocator's cost over the base's, with the geometric
standard deviation across those jobs (population form). xmalloc-test is left out: its medians
compare modes. Then the same for memory, the median cgroup peak, over the jobs where the base's peak
reaches 30 MiB, since mimalloc's huge pages put its own over that on every job. After the tables, each job the base loses to its best incumbent, worst
first, in time and then in memory."""
import argparse
import collections
import json
import math
import statistics


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("files", nargs="+")
    ap.add_argument("--base", default="cralloc")
    ap.add_argument("--allocators", default="jemalloc,tcmalloc-percpu,mimalloc,glibc,snmalloc")
    a = ap.parse_args()
    others = a.allocators.split(",")
    cost = collections.defaultdict(lambda: collections.defaultdict(list))
    peak = collections.defaultdict(lambda: collections.defaultdict(list))
    group = {}
    names = {}
    for path in a.files:
        for line in open(path):
            r = json.loads(line)
            p = r.get("primary") or {}
            if r.get("type") != "run" or not r.get("valid") or not p.get("value"):
                continue
            job = f"{r['job']} ({r['mode']})"
            cost[job][r["allocator"]].append(p["value"] if p.get("better", "lower") == "lower" else 1 / p["value"])
            group[job] = r["group"]
            if (r.get("cgroup") or {}).get("peak"):
                peak[job][r["allocator"]].append(r["cgroup"]["peak"])
            names[job] = p.get("name", "")
    ratios = collections.defaultdict(lambda: collections.defaultdict(list))
    behind = []
    for job, per in cost.items():
        if a.base not in per or "xmalloc-test" in job:
            continue
        base = statistics.median(per[a.base])
        for other in others:
            if other in per:
                ratios[group[job]][other].append(statistics.median(per[other]) / base)
        incumbents = {o: statistics.median(v) for o, v in per.items() if o != a.base and "ctl" not in o}
        if incumbents:
            best = min(incumbents, key=incumbents.get)
            if incumbents[best] < base:
                behind.append((base / incumbents[best], job, best, names[job]))
    print("| Group (jobs) | " + " | ".join(others) + " |")
    print("|---|" + "---:|" * len(others))
    for g, per in ratios.items():
        cells = []
        for other in others:
            v = per.get(other)
            if not v:
                cells.append("")
                continue
            logs = [math.log(x) for x in v]
            cells.append(f"{math.exp(statistics.mean(logs)):.2f} (x{math.exp(statistics.pstdev(logs)):.2f})")
        print(f"| {g} ({max(len(x) for x in per.values())}) | " + " | ".join(cells) + " |")
    print()
    heavier = []
    mem = collections.defaultdict(list)
    for job, per in peak.items():
        if a.base not in per:
            continue
        base = statistics.median(per[a.base])
        for other in others:
            if other in per and base >= 30 << 20:
                mem[other].append(statistics.median(per[other]) / base)
        incumbents = {o: statistics.median(v) for o, v in per.items() if o != a.base and "ctl" not in o}
        if incumbents and base >= 30 << 20:
            best = min(incumbents, key=incumbents.get)
            if incumbents[best] < base:
                heavier.append((base / incumbents[best], job, best, base / 2**20, incumbents[best] / 2**20))
    cells = []
    for other in others:
        logs = [math.log(x) for x in mem.get(other, [])]
        cells.append(f"{math.exp(statistics.mean(logs)):.2f} (x{math.exp(statistics.pstdev(logs)):.2f}, {len(logs)})" if logs else "")
    print("Memory, jobs at 30 MiB or more | " + " | ".join(cells) + " |")
    print()
    for ratio, job, best, name in sorted(behind, reverse=True):
        print(f"{ratio:6.2f}  {job}: {best} ({name})")
    print()
    for ratio, job, best, b, o in sorted(heavier, reverse=True):
        print(f"{ratio:6.2f}  {job}: {best}, {b:.0f} MiB against {o:.0f}")


main()
