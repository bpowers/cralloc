#!/usr/bin/env python3
"""Counts a server's memory system calls (strace -f -ttt -T output) per segment of a loadgen
plan: calls, total and largest time in the call; and, for bursts after silence, in the first
HEAD ms against the rest.

  strace_phases.py RUN_DIR [HEAD_MS=50]
RUN_DIR holds strace.txt, summary.json, and serve.py's record of the run in ../../runs.jsonl."""
import collections, json, re, sys
from pathlib import Path

d = Path(sys.argv[1]).resolve(); head_ms = float(sys.argv[2]) if len(sys.argv) > 2 else 50
summ = json.load(open(d / "summary.json"))
rec = next(json.loads(l) for l in open(d.parent.parent / "runs.jsonl") if json.loads(l).get("dir") == str(d))
off = rec["realtime_minus_monotonic_ns"]
t0 = summ["start_mono_ns"]
segs = summ["segments"]

line_re = re.compile(r"^(\d+)\s+(\d+\.\d+)\s+(?:<\.\.\. )?(\w+)(.*?)(?:<(\d+\.\d+)>)?$")
calls = []
pending = {}
for line in open(d / "strace.txt"):
    m = line_re.match(line.strip())
    if not m:
        continue
    pid, ts, name, rest, dur = m.groups()
    t = int(float(ts) * 1e9) - off - t0  # ns from the generator's start
    if "unfinished" in rest:
        pending[(pid, name)] = t
        continue
    if "resumed" in rest:
        t = pending.pop((pid, name), t)
    calls.append((t / 1e6, name, float(dur) * 1e6 if dur else 0.0, rest))


def phase(t_ms):
    for i, s in enumerate(segs):
        if s["begin_ms"] <= t_ms < s["end_ms"]:
            name = s["name"]
            if name == "on" and i > 0 and segs[i - 1]["name"] == "off":
                return "on-head" if t_ms - s["begin_ms"] < head_ms else "on-rest"
            return name
    return "before" if t_ms < 0 else "after"


stats = collections.defaultdict(lambda: [0, 0.0, 0.0])
for t, name, dur, _ in calls:
    k = (phase(t), name)
    st = stats[k]
    st[0] += 1
    st[1] += dur
    st[2] = max(st[2], dur)
print(f"{'phase':10s} {'call':12s} {'count':>7s} {'total us':>10s} {'max us':>8s}")
order = ["before"] + [s["name"] for s in segs] + ["on-head", "on-rest", "after"]
seen = []
for p in order:
    if p in seen:
        continue
    seen.append(p)
    for (ph, name), (n, tot, mx) in sorted(stats.items()):
        if ph == p:
            print(f"{ph:10s} {name:12s} {n:7d} {tot:10.0f} {mx:8.0f}")
slow = sorted((c for c in calls if c[2] > 200), key=lambda c: -c[2])[:15]
print("\nslowest calls:")
for t, name, dur, rest in slow:
    print(f"  {t:10.1f} ms {phase(t):8s} {name} {dur:.0f} us {rest[:70]}")
