"""asmalloc against the best incumbent per job: compare.py [--asm NAME] FILE... (jsonl or .gz).

The best incumbent is the incumbent (not asmalloc, not a control) with the lowest median cost;
the gap is the median of per-round ratios of asmalloc's cost to it, with a 95% bootstrap
interval over those ratios: "level" when the interval includes 1. Cost is time (lower is
better) or the inverse of a rate. Memory is the cgroup peak against the smallest incumbent and
against jemalloc, the same way. Prints a table, the counts and the geometric mean of the time
ratios over every job but xmalloc-test. --json OUT writes the per-job results."""
import argparse
import gzip
import json
import math
import random
import statistics
from collections import defaultdict


def records(path):
    opener = gzip.open if path.endswith(".gz") else open
    with opener(path, "rt") as f:
        for line in f:
            line = line.strip()
            if line:
                yield path, json.loads(line)


def boot(ratios, seed):
    rng = random.Random(seed)
    meds = sorted(statistics.median(rng.choices(ratios, k=len(ratios))) for _ in range(2000))
    return meds[50], meds[1949]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("files", nargs="+")
    ap.add_argument("--asm", default="asmalloc")
    ap.add_argument("--json")
    ap.add_argument("--exclude", default="")
    a = ap.parse_args()
    excluded = set(x for x in a.exclude.split(",") if x)
    cost = defaultdict(lambda: defaultdict(dict))  # job -> alloc -> roundkey -> cost
    peak = defaultdict(lambda: defaultdict(dict))
    metric = {}
    controls = set()
    for path, r in (rec for f in a.files for rec in records(f)):
        if r.get("type") != "run" or not r.get("valid", False):
            continue
        if r.get("control_of"):
            controls.add(r["allocator"])
        p = r.get("primary") or {}
        if "value" not in p or not p["value"]:
            continue
        job = f"{r['job']} ({r['mode']})"
        key = (path, r.get("session", {}).get("id") if isinstance(r.get("session"), dict) else r.get("session"), r["round"])
        v = p["value"]
        c = v if p.get("better", "lower") == "lower" else 1.0 / v
        cost[job][r["allocator"]][key] = c
        cg = (r.get("cgroup") or {}).get("peak")
        if cg:
            peak[job][r["allocator"]][key] = cg
        metric[job] = p.get("name", "")
    rows = []
    for i, job in enumerate(sorted(cost)):
        per = cost[job]
        if a.asm not in per:
            continue
        inc = [x for x in per if x != a.asm and x not in controls and x not in excluded]
        if not inc:
            continue

        def compare(table, against, seed):
            asm = table[a.asm]
            ratios = [asm[k] / table[against][k] for k in asm if k in table[against]]
            if len(ratios) < 3:
                return None
            lo, hi = boot(ratios, seed)
            return statistics.median(ratios), lo, hi

        best = min(inc, key=lambda x: statistics.median(per[x].values()))
        t = compare(per, best, i)
        pinc = [x for x in inc if x in peak[job] and peak[job][x]]
        small = min(pinc, key=lambda x: statistics.median(peak[job][x].values())) if pinc else None
        m = compare(peak[job], small, i + 7) if small and a.asm in peak[job] else None
        je = compare(peak[job], "jemalloc", i + 13) if "jemalloc" in peak[job] and a.asm in peak[job] else None
        rows.append({
            "job": job, "metric": metric[job], "best": best,
            "asm_med": statistics.median(per[a.asm].values()), "best_med": statistics.median(per[best].values()),
            "time": t, "smallest": small,
            "asm_peak": statistics.median(peak[job][a.asm].values()) / 2**20 if a.asm in peak[job] else None,
            "small_peak": statistics.median(peak[job][small].values()) / 2**20 if small else None,
            "mem": m, "je": je,
        })

    def word(c):
        if c is None:
            return "-"
        med, lo, hi = c
        if lo <= 1 <= hi:
            return f"level ({100 * (med - 1):+.0f}%)"
        return f"ahead {100 * (1 - med):.0f}%" if med < 1 else f"behind {100 * (med - 1):.0f}%"

    ahead = level = behind = 0
    logs = []
    for r in rows:
        w = word(r["time"])
        if "xmalloc-test" not in r["job"] and r["time"]:
            logs.append(math.log(r["time"][0]))
            if w.startswith("ahead"):
                ahead += 1
            elif w.startswith("level"):
                level += 1
            else:
                behind += 1
        print(f"| {r['job']} | {r['best']} | {w} | {word(r['mem'])} vs {r['smallest']} "
              f"({r['asm_peak'] or 0:.1f} / {r['small_peak'] or 0:.1f} MiB) | {word(r['je'])} |")
    gm = math.exp(sum(logs) / len(logs)) if logs else float("nan")
    print(f"ahead {ahead}, level {level}, behind {behind} of {len(logs)}; geomean {gm:.3f}")
    if a.json:
        with open(a.json, "w") as f:
            json.dump(rows, f)


main()
