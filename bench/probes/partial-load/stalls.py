#!/usr/bin/env python3
"""Where a server's slow requests come from, from loadgen's raw records (run.py --raw).

A request is slow when its latency is over SLOW_US. Slow requests are grouped into events:
runs of slow requests whose due times are within GAP_MS of each other. An event that slows
many requests at once is a stall of the server (or of the generator); a slow request alone is
a request that was slow on its own. Prints the latency percentiles, how many slow requests
fall in events of each size, and the largest events with when they began, so they can be
lined up with a trace of the server.

  stalls.py FILE.raw [--slow-us 60] [--gap-ms 0.5] [--segment 2] [--top 15] [--calls]

--calls reads run.py madvise's record beside the raw file (<tag>.madvise, <tag>.clocks and
<tag>.result.json) and counts the slow requests in events during which, or in the 2 ms before
which, the server made a memory system call; the largest events list those calls.
"""
import argparse
import json
import struct
from array import array

ap = argparse.ArgumentParser()
ap.add_argument("raw")
ap.add_argument("--slow-us", type=float, default=60.0)
ap.add_argument("--gap-ms", type=float, default=0.5)
ap.add_argument("--segment", type=int, default=2)
ap.add_argument("--top", type=int, default=15)
ap.add_argument("--calls", action="store_true")
a = ap.parse_args()

data = open(a.raw, "rb").read()
due, lat = array("Q"), array("Q")
for d, l, seg, _flags in struct.iter_unpack("<QQHH", data):
    if seg == a.segment:
        due.append(d)
        lat.append(l)
n = len(lat)
s = sorted(lat)
pct = {p: s[min(n - 1, int(n * p))] / 1e3 for p in (0.5, 0.99, 0.999, 0.9999)}
slow_ns, gap_ns = a.slow_us * 1e3, a.gap_ms * 1e6
events = []  # [first due, last due, count, worst]
for d, l in zip(due, lat):
    if l <= slow_ns:
        continue
    if events and d - events[-1][1] <= gap_ns:
        e = events[-1]
        e[1], e[2], e[3] = d, e[2] + 1, max(e[3], l)
    else:
        events.append([d, d, 1, l])
slow = sum(e[2] for e in events)
t0 = due[0] if n else 0
print(f"{a.raw}: {n} requests over {(due[-1] - t0) / 1e9:.1f} s; p50 {pct[0.5]:.0f} p99 {pct[0.99]:.0f} "
      f"p99.9 {pct[0.999]:.0f} p99.99 {pct[0.9999]:.0f} us; {slow} over {a.slow_us:.0f} us "
      f"({slow / max(n, 1) * 1e3:.2f} per 1,000) in {len(events)} events")
for lo, hi in ((1, 1), (2, 9), (10, 99), (100, 999), (1000, 10**12)):
    es = [e for e in events if lo <= e[2] <= hi]
    k = sum(e[2] for e in es)
    print(f"  events of {lo}-{hi if hi < 10**12 else 'more'} slow requests: {len(es):6d} events, {k:7d} requests "
          f"({k / max(slow, 1) * 100:5.1f}%)")
calls = []  # (start, end, name) in ns of the generator's clock, as due times are
if a.calls:
    tag = a.raw[: -len(".raw")]
    wall0, mono0 = open(tag + ".clocks").read().split()
    start = json.loads(open(tag + ".result.json").read())["start_mono_ns"]
    # A call another thread interrupted is split over an `<unfinished ...>` line, which has its
    # start, and a `<... resumed>` line, which has its duration.
    pending = {}
    for line in open(tag + ".madvise"):
        f = line.split()
        if len(f) < 3:
            continue
        if "<unfinished" in line:
            pending[f[0]] = (float(f[1]), f[2].split("(")[0])
            continue
        if "resumed>" in line:
            if f[0] not in pending or not f[-1].startswith("<"):
                continue
            t, name = pending.pop(f[0])
        elif "(" in f[2] and f[-1].startswith("<"):
            t, name = float(f[1]), f[2].split("(")[0]
        else:
            continue
        at = (t - float(wall0)) * 1e9 + int(mono0) - start
        calls.append((at, at + float(f[-1].strip("<>")) * 1e9, name))
    calls.sort()
    t_first, t_last = due[0], due[-1]
    inside = [c for c in calls if t_first <= c[0] <= t_last]
    by = {}
    for c in inside:
        by.setdefault(c[2], []).append(c[1] - c[0])
    print("  calls in the segment: " + ", ".join(
        f"{k} {len(v)} (median {sorted(v)[len(v) // 2] / 1e3:.0f} us, max {max(v) / 1e3:.0f} us)" for k, v in by.items()))


def near(e):
    return [c for c in calls if c[0] <= e[1] and c[1] >= e[0] - 2e6]


if a.calls:
    hit = sum(e[2] for e in events if near(e))
    print(f"  slow requests in events near a call: {hit} ({hit / max(slow, 1) * 100:.1f}%)")
big = sorted(events, key=lambda e: -e[2])[: a.top]
if big:
    print("  largest: start s, length ms, slow requests, worst us" + (", calls near" if a.calls else ""))
    for e in sorted(big):
        cs = " ".join(f"{c[2]}:{(c[1] - c[0]) / 1e3:.0f}us" for c in near(e)) if a.calls else ""
        print(f"    {(e[0] - t0) / 1e9:8.3f}  {(e[1] - e[0]) / 1e6:7.2f}  {e[2]:6d}  {e[3] / 1e3:8.0f}  {cs}")
