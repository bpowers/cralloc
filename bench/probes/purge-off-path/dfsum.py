#!/usr/bin/env python3
"""Summarizes a directory of dftrace.py runs: peak RssAnon of each run, and the medians of
the allocator thread's wake-ups, CPU time, run-queue wait and involuntary switches over a run.

  dfsum.py DIR
"""
import glob
import json
import statistics
import sys

peaks, wakes, cpu, invol, wait, longest = [], [], [], [], [], []
for f in sorted(glob.glob(f"{sys.argv[1]}/run*.json")):
    t = [s for s in json.load(open(f)) if s[1] is not None]
    peaks.append(max(s[1] for s in t) >> 10)
    th = [s for s in t if s[4] is not None]
    if th:
        wakes.append(th[-1][4] - th[0][4])
        cpu.append((th[-1][2] - th[0][2]) / 1e6)
        wait.append((th[-1][3] - th[0][3]) / 1e6)
        longest.append(max((b[3] - a[3]) / 1e6 for a, b in zip(th, th[1:])))
        if len(th[0]) > 5 and th[0][5] is not None:
            invol.append(th[-1][5] - th[0][5])
print(f"runs {len(peaks)}  peak MiB sorted {sorted(peaks)}")
if wakes:
    print(f"thread wake-ups a run, median {statistics.median(wakes)}; CPU ms a run, median {statistics.median(cpu):.1f}")
    print(f"thread run-queue wait ms a run, median {statistics.median(wait):.1f}; most in one 5 ms sample, "
          f"median {statistics.median(longest):.2f}, max {max(longest):.2f}")
if invol:
    print(f"thread involuntary switches a run (yields that gave the CPU away, and preemptions), median {statistics.median(invol)}")
