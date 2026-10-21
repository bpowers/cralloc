#!/usr/bin/env python3
"""A request timeline from loadgen's raw records (run.py --raw): for each bucket of BUCKET_MS
of due time in the measured segment, the requests, the largest latency and how many missed
their slot. Shows when a server stalls and for how long.

  timeline.py FILE.raw [BUCKET_MS] [SEGMENT]
"""
import struct
import sys

path = sys.argv[1]
bucket_ms = float(sys.argv[2]) if len(sys.argv) > 2 else 100.0
segment = int(sys.argv[3]) if len(sys.argv) > 3 else 2
buckets = {}
with open(path, "rb") as f:
    data = f.read()
for due, lat, seg, flags in struct.iter_unpack("<QQHH", data):
    if seg != segment:
        continue
    b = int(due / 1e6 // bucket_ms)
    n, worst, missed = buckets.get(b, (0, 0, 0))
    buckets[b] = (n + 1, max(worst, lat), missed + (flags & 1))
for b in sorted(buckets):
    n, worst, missed = buckets[b]
    flag = "  <<" if worst > 1_000_000 else ""
    print(f"{b * bucket_ms / 1000:8.1f} s  {n:7d} req  max {worst / 1000:10.0f} us  missed {missed:6d}{flag}")
