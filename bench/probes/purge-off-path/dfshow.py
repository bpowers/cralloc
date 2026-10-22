#!/usr/bin/env python3
"""Shows a dftrace.py run in steps of 50 ms: RssAnon, and the allocator thread's CPU time,
run-queue wait and wake-ups in each step.

  dfshow.py RUN.json [FROM TO]
"""
import json
import sys

t = [s for s in json.load(open(sys.argv[1])) if s[1] is not None]
lo = float(sys.argv[2]) if len(sys.argv) > 2 else 0
hi = float(sys.argv[3]) if len(sys.argv) > 3 else 1e9
prev = None
for s in t:
    if prev is not None and s[0] - prev[0] < 0.05:
        continue
    if prev is not None and lo <= s[0] <= hi and s[2] is not None and prev[2] is not None:
        print(f"{s[0]:6.3f} rss {s[1] >> 10:5d} MiB  thread ran {(s[2] - prev[2]) / 1e6:6.2f} ms"
              f"  waited {(s[3] - prev[3]) / 1e6:6.2f} ms  woke {s[4] - prev[4]}")
    prev = s
