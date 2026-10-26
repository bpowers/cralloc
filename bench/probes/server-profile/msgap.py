#!/usr/bin/env python3
"""The allocator's belief against the kernel's, over a server run with the memstat sampler
(bench-alloc's `asmalloc-memstat`, writing to `ASMALLOC_MEMSTAT.<pid>`):

  msgap.py MEMSTAT_PREFIX [THP_JSONL --build NAME] [--every 5]

For the process with the largest RSS among PREFIX.<pid>, prints every EVERY seconds: RSS,
the heap the walk saw resident, the live bytes, `free` (free slots on pages the kernel says
are resident, by mincore), `retained` (the back end's own count of free bytes it believes
resident) and the gap `free - retained`: free memory the allocator thinks it has returned
that is resident again, such as huge pages khugepaged rebuilt. With THP_JSONL (thpstat.py's
output for the same run), the run cgroup's cumulative `thp_collapse_alloc` beside it. Then
the `peak`/`final` dumps' AnonHugePages.
"""
import argparse
import glob
import json
import re

MIB = 1 << 20


def samples(path):
    out = []
    for line in open(path, errors="replace"):
        if line.startswith("sample "):
            out.append({k: int(v) for k, v in re.findall(r"(\w+)=(-?\d+)", line)})
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("prefix")
    ap.add_argument("thp", nargs="?")
    ap.add_argument("--every", type=float, default=5)
    ap.add_argument("--build", default="", help="the bench build whose THP_JSONL rows to read")
    a = ap.parse_args()
    best, path = None, None
    for p in glob.glob(a.prefix + ".*"):
        s = samples(p)
        top = max((x["rss"] for x in s), default=0)
        if best is None or top > best:
            best, path = top, p
    s = samples(path)
    series = []
    if a.thp:
        rows = [json.loads(l) for l in open(a.thp)]
        rows = [r for r in rows if r.get("series") and f"/{a.build}/" in r["exe"]]
        if rows:
            series = max(rows, key=lambda r: r["series"][-1][1])["series"]

    def collapses(t):
        base = series[0][1] if series else 0
        v = base
        for ts, n in series:
            if ts <= t:
                v = n
        return v - base

    print(f"{path}: {len(s)} samples")
    print(f"{'t s':>6} {'rss':>6} {'heap':>6} {'live':>6} {'free':>6} {'retain':>6} {'gap':>6} {'coll':>5}")
    nxt, gaps = 0, []
    for x in s:
        t = x["t_ms"] / 1000
        gap = x["free"] - x["retained"]
        gaps.append((t, gap))
        if t >= nxt:
            nxt = t + a.every
            print(f"{t:6.0f} {x['rss'] / MIB:6.0f} {x['heap'] / MIB:6.0f} {x['live'] / MIB:6.0f} "
                  f"{x['free'] / MIB:6.0f} {x['retained'] / MIB:6.0f} {gap / MIB:6.1f} {collapses(t):5d}")
    if gaps:
        print(f"gap MiB: max {max(g for _, g in gaps) / MIB:.1f}, last {gaps[-1][1] / MIB:.1f}")
    for line in open(path, errors="replace"):
        if line.startswith(("peak ", "peakrss ", "final ")):
            d = dict(re.findall(r"(\w+)=(-?\d+)", line))
            print(f"{line.split()[0]}: rss {int(d['rss']) / MIB:.0f} thp {int(d['thp']) / MIB:.0f} "
                  f"free {int(d['free']) / MIB:.0f} retained {int(d['retained']) / MIB:.0f} MiB")


if __name__ == "__main__":
    main()
