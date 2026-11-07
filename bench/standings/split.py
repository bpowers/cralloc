#!/usr/bin/env python3
"""Where a gap to another allocator lies: split.py [--base NAME] [--against NAME] FILE...

For each job on which the base costs more than the other (by default each job's cheapest incumbent),
over the job's measured work (the workload's phases, a server's window, or else the whole process),
medians over valid runs: the ratios of cycles, user cycles and instructions, and how much of the gap
in cycles instructions explain, log(instruction ratio) / log(cycle ratio) as in
docs/research/measuring.md, "The IPC split of a gap". The rest is stalls, or kernel cycles where user
cycles are level. Faults are per run. An instruction count is the whole work's, so a stall can hide
behind fast added code."""
import argparse
import collections
import json
import math
import statistics


def counts(r):
    phases = (r.get("workload") or {}).get("phases") or {}
    if phases:
        sums = collections.Counter()
        for p in phases.values():
            for k in ("cycles", "cycles_user", "instructions", "page_faults"):
                sums[k] += p.get(k, 0)
        return sums
    return r.get("window") or r.get("process") or {}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("files", nargs="+")
    ap.add_argument("--base", default="cralloc")
    ap.add_argument("--against")
    a = ap.parse_args()
    runs = collections.defaultdict(lambda: collections.defaultdict(list))
    for path in a.files:
        for line in open(path):
            r = json.loads(line)
            if r.get("type") == "run" and r.get("valid") and (r.get("primary") or {}).get("value"):
                runs[f"{r['job']} ({r['mode']})"][r["allocator"]].append(r)

    def med(rs, f):
        v = [x for x in map(f, rs) if x is not None]
        return statistics.median(v) if v else None

    def cost(r):
        p = r["primary"]
        return p["value"] if p.get("better", "lower") == "lower" else 1 / p["value"]

    print(f"{'ratio':>6} {'job':46} {'other':16} {'cycles':>7} {'user':>7} {'instr':>7} {'share':>6} {'faults':>16}")
    rows = []
    for job, per in runs.items():
        if a.base not in per:
            continue
        others = {o: med(rs, cost) for o, rs in per.items() if o != a.base and "ctl" not in o}
        other = a.against if a.against in others else (min(others, key=others.get) if others else None)
        if other is None or med(per[a.base], cost) <= others[other]:
            continue
        rows.append((med(per[a.base], cost) / others[other], job, other))
    for ratio, job, other in sorted(rows, reverse=True):
        base, against = runs[job][a.base], runs[job][other]
        c = {
            k: (med(base, lambda r: counts(r).get(k)), med(against, lambda r: counts(r).get(k)))
            for k in ("cycles", "cycles_user", "instructions", "page_faults")
        }
        over = lambda k: c[k][0] / c[k][1] if c[k][0] and c[k][1] else float("nan")
        cy, ins = over("cycles"), over("instructions")
        share = math.log(ins) / math.log(cy) if cy > 0 and cy != 1 and ins > 0 else float("nan")
        faults = f"{c['page_faults'][0] or 0:.0f}/{c['page_faults'][1] or 0:.0f}"
        print(
            f"{ratio:6.3f} {job:46} {other:16} {cy:7.3f} {over('cycles_user'):7.3f} {ins:7.3f} {share:6.2f} {faults:>16}"
        )


main()
