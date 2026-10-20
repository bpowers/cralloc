#!/usr/bin/env python3
"""The start of each burst after silence, per run: how long until latency is back to its
steady level, and how much waiting the start added in all.

  bursts.py DIR [DIR...]

For every load segment that follows silence: the steady p50 is that of the segment's
requests due after its first 100 ms; recovery is the due time after which no request in the
next millisecond waits more than three times that; the excess is the sum over the segment's
first 20 ms of latency above the steady p50, in request-milliseconds. Medians over the bursts
of a run, then the median and range over runs, by allocator. PER_RUN=1 also prints each run."""
import collections, json, os, statistics, struct, sys
from pathlib import Path

by = collections.defaultdict(lambda: collections.defaultdict(list))
for d in sys.argv[1:]:
    for line in open(Path(d) / "runs.jsonl"):
        r = json.loads(line)
        if not r.get("ok"):
            continue
        run = Path(r["dir"])
        segs = r["summary"]["segments"]
        after = [i for i, s in enumerate(segs) if s["name"] == "on" and segs[i - 1]["name"] == "off"]
        per = collections.defaultdict(list)
        for due, lat, seg, fl in struct.iter_unpack("<QQHH", (run / "raw.bin").read_bytes()):
            if seg in after:
                per[seg].append((due / 1e6 - segs[seg]["begin_ms"], lat / 1e3))
        rec, exc, first = [], [], []
        for seg, v in per.items():
            v.sort()
            steady = statistics.median(l for t, l in v if t >= 100)
            bad = [t for t, l in v if t < 100 and l > 3 * steady]
            rec.append(max(bad) if bad else 0.0)
            exc.append(sum(max(0.0, l - steady) for t, l in v if t < 20) / 1e3)
            first.append(statistics.median(l for t, l in v if t < 0.5))
        key = r["server"]
        if os.environ.get("PER_RUN"):
            print(r["server"], r["alloc"], r["round"], [round(statistics.median(x), 2) for x in (rec, exc, first)])
        by[key][r["alloc"]].append((statistics.median(rec), statistics.median(exc), statistics.median(first)))
for srv, allocs in sorted(by.items()):
    print(f"\n## {srv}: per burst after silence (median over bursts; median [range] over runs)")
    print(f"{'alloc':12s} {'runs':>4s} {'recovery ms':>18s} {'excess req-ms':>18s} {'first 0.5 ms p50 us':>22s}")
    for a, xs in sorted(allocs.items()):
        cols = []
        for k in range(3):
            v = [x[k] for x in xs]
            cols.append(f"{statistics.median(v):6.2f} [{min(v):.2f}-{max(v):.2f}]")
        print(f"{a:12s} {len(xs):4d} {cols[0]:>18s} {cols[1]:>18s} {cols[2]:>22s}")
