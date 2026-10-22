#!/usr/bin/env python3
"""What the floor's comparison read against what a purge could return, through a run of the
servers-thread plan, from the memstat build's breakdowns (`diagm-*` entries: a full breakdown
every second, ASMALLOC_MEMSTAT_EACH=1) lined up with the run's plan by the watcher.

  memsum.py DIR TAG

For each run, at the end of the load, 10 s and 29 s into the silence, and 10 and 59 s after
close: the retained count (what the floor is compared with), the free bytes on resident pages,
of them those on wholly free pages (what a purge can return), small and large classes apart,
and the classes holding the most returnable memory.
"""
import json
import re
import sys
from pathlib import Path

SMALL_MAX = 32 << 10
MiB = 2 ** 20
FILL = {"connect store 1M thread": 3, "orders axum cache 2G thread": 6, "meilisearch hn thread": 5}


def blocks(path):
    """Each `at` breakdown: (t_ms, retained, {size: (free_res, free_whole)})."""
    out, cur = [], None
    for line in open(path, errors="replace"):
        if line.startswith("at "):
            kv = dict(re.findall(r"(\w+)=(\S+)", line))
            cur = (int(kv["t_ms"]), int(kv["retained"]), {})
            out.append(cur)
        elif line.startswith("at-class ") and cur is not None:
            kv = dict(re.findall(r"(\w+)=(\S+)", line))
            size = int(kv["size"])
            free_res = int(kv["free"].split("/")[0])
            cur[2][size] = (cur[2].get(size, (0, 0))[0] + free_res, cur[2].get(size, (0, 0))[1] + int(kv["free_whole"]))
    return out


def main():
    d, tag = Path(sys.argv[1]), sys.argv[2]
    runs = [json.loads(l) for l in open(d / f"{tag}.jsonl") if '"type":"run"' in l]
    watch = [json.loads(l) for l in open(d / f"{tag}.watch.jsonl")]
    procs = [w for w in watch if w["type"] == "proc"]
    gens = [w for w in watch if w["type"] == "gen"]
    for r in runs:
        p = [p for p in procs if abs(p["start"] - r["started"]) < 3]
        if not p:
            continue
        pid, start = p[0]["pid"], p[0]["start"]
        gen = min(g["start"] for g in gens if g["start"] >= start)
        files = list((d / "memstat").glob(f"*.{pid}"))
        if not files:
            print(f"{r['allocator']}: no memstat file for {pid}")
            continue
        bl = blocks(files[0])
        if r.get("served"):
            segs = r["served"]["generator"]["segments"]
            begin = lambda n: [s for s in segs if s["name"] == n][0]["begin_ms"] / 1e3
            s70_end = [s for s in segs if s["name"] == "s70"][-1]["end_ms"] / 1e3
        else:
            # An invalid run keeps no plan: the suite's, from the job's fill.
            fill = FILL[r["job"]]
            s70_end = fill + 2 + 30
            begin = {"off": s70_end, "idle": s70_end + 30 + 2}.get
        plan0 = (gen - start) * 1e3
        print(f"\n=== {tag} {r['allocator']} (pid {pid}); MiB: retained (floor's input) / free resident / of it on wholly free pages; small | large")
        for name, t in (("end of load", s70_end), ("silence +10 s", begin("off") + 10), ("silence +29 s", begin("off") + 29),
                        ("close +10 s", begin("idle") + 10), ("close +59 s", begin("idle") + 59)):
            want = plan0 + t * 1e3
            b = max((x for x in bl if x[0] <= want), key=lambda x: x[0], default=None)
            if b is None:
                continue
            small = [v for s, v in b[2].items() if s <= SMALL_MAX]
            large = [v for s, v in b[2].items() if s > SMALL_MAX]
            fs, ws = sum(v[0] for v in small), sum(v[1] for v in small)
            fl, wl = sum(v[0] for v in large), sum(v[1] for v in large)
            top = sorted(b[2].items(), key=lambda kv: -kv[1][1])[:5]
            print(f"  {name:14s} retained {b[1] / MiB:7.1f}; small {fs / MiB:7.1f} / {ws / MiB:6.1f} | large {fl / MiB:7.1f} / {wl / MiB:6.1f};"
                  f" most returnable: " + ", ".join(f"{s} B {v[1] / MiB:.1f}" for s, v in top if v[1]))


if __name__ == "__main__":
    main()
