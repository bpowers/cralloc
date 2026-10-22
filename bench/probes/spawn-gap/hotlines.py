#!/usr/bin/env python3
"""Samples of one event inside the functions matching PATTERN, by 64-byte line of the address
space, with each line's offset from the function's start. Shows which code lines are hot and
where they fall relative to line boundaries.
usage: hotlines.py EVENT DATA PATTERN [TOP]"""
import collections, subprocess, sys

event, data, pat = sys.argv[1], sys.argv[2], sys.argv[3]
top = int(sys.argv[4]) if len(sys.argv) > 4 else 20
out = subprocess.run(["perf", "script", "-i", data, "-F", "event,ip,sym,symoff", "--comms", "tokio-rt-worker"],
                     capture_output=True, text=True).stdout
lines = collections.Counter()
total = 0
for line in out.splitlines():
    head, _, rest = line.partition(":")
    if event not in head or pat not in rest:
        continue
    f = rest.split(None, 1)
    ip = int(f[0], 16)
    off = int(f[1].rsplit("+0x", 1)[1].split()[0], 16) if "+0x" in f[1] else 0
    lines[(ip & ~63, (ip & ~63) - (ip - off))] += 1
    total += 1
print(f"{total} samples of {event} in {pat}")
for (line, start), n in lines.most_common(top):
    print(f"  line {line:#x} (function+{start:5}) {n:7}")
