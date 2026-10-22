#!/usr/bin/env python3
"""Summarizes schedtrace.py files, per server process (named by the build directory of its
executable): over the samples where the server's other threads ran at all, the run-queue wait
the server's threads other than the allocator's had in each sample interval (about 2 ms; p99,
p99.9 and max of the per-interval sum, in us), and the allocator thread's bursts: CPU time in
one interval (p99, max) and the longest run of consecutive intervals it was on a CPU.

  schedsum.py FILE...
"""
import json
import sys
from collections import defaultdict


def pct(v, q):
    v = sorted(v)
    return v[min(len(v) - 1, int(q * len(v)))] if v else 0


runs = defaultdict(list)
for path in sys.argv[1:]:
    for line in open(path):
        s = json.loads(line)
        runs[(s["pid"], s["exe"])].append(s)

print("| build | pid | intervals | workers' wait us a 2 ms: p99 / p99.9 / max | thread ran us a 2 ms: p99 / max | thread's longest burst, ms |")
print("|---|---:|---:|---|---|---:|")
for (pid, exe), samples in sorted(runs.items(), key=lambda kv: kv[0][0]):
    build = exe.split("/target/")[1].split("/")[0] if "/target/" in exe else exe
    waits, ran = [], []
    burst = longest = 0.0
    for a, b in zip(samples, samples[1:]):
        w, worked = 0, False
        thread_ran = 0
        for tid, (name, run, wait) in b["tasks"].items():
            old = a["tasks"].get(tid)
            if old is None:
                continue
            if name == "asmalloc":
                thread_ran = run - old[1]
            else:
                w += wait - old[2]
                worked |= run > old[1]
        if not worked:
            continue
        waits.append(w / 1e3)
        ran.append(thread_ran / 1e3)
        burst = burst + thread_ran / 1e6 if thread_ran > 0 else 0.0
        longest = max(longest, burst)
    print(f"| {build} | {pid} | {len(waits)} | {pct(waits, .99):.0f} / {pct(waits, .999):.0f} / "
          f"{max(waits, default=0):.0f} | {pct(ran, .99):.0f} / {max(ran, default=0):.0f} | {longest:.2f} |")
