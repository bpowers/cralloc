#!/usr/bin/env python3
"""Summarizes a directory of dftrace.py runs: peak RssAnon of each run, and the medians of
the allocator thread's wake-ups and CPU time over a run.

  dfsum.py DIR
"""
import glob
import json
import statistics
import sys

peaks, wakes, cpu = [], [], []
for f in sorted(glob.glob(f"{sys.argv[1]}/run*.json")):
    t = [s for s in json.load(open(f)) if s[1] is not None]
    peaks.append(max(s[1] for s in t) >> 10)
    th = [s for s in t if s[4] is not None]
    if th:
        wakes.append(th[-1][4] - th[0][4])
        cpu.append((th[-1][2] - th[0][2]) / 1e6)
print(f"runs {len(peaks)}  peak MiB sorted {sorted(peaks)}")
if wakes:
    print(f"thread wake-ups a run, median {statistics.median(wakes)}; CPU ms a run, median {statistics.median(cpu):.1f}")
