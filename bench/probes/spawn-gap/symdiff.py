#!/usr/bin/env python3
"""Samples of one event by symbol in two perf.data files, side by side, largest difference
first. usage: symdiff.py EVENT A.data B.data [TOP]"""
import collections, subprocess, sys

event, a, b = sys.argv[1], sys.argv[2], sys.argv[3]
top = int(sys.argv[4]) if len(sys.argv) > 4 else 25


def counts(path):
    out = subprocess.run(["perf", "script", "-i", path, "-F", "event,ip,sym,dso", "--comms", "tokio-rt-worker"],
                         capture_output=True, text=True).stdout
    c = collections.Counter()
    for line in out.splitlines():
        parts = line.split(":", 1)
        if len(parts) < 2 or event not in parts[0]:
            continue
        rest = parts[1].strip()
        head = rest.split(" (")[0].split(None, 1)
        sym = head[1].split("+0x")[0].strip() if len(head) > 1 else "?"
        dso = rest.rsplit("(", 1)[-1].rstrip(")").split("/")[-1] if "(" in rest else "?"
        c[(dso, sym[:90])] += 1
    return c


ca, cb = counts(a), counts(b)
print(f"{event}: {sum(ca.values())} in {a.split('/')[-1]}, {sum(cb.values())} in {b.split('/')[-1]}")
keys = sorted(set(ca) | set(cb), key=lambda k: -abs(ca[k] - cb[k]))
for k in keys[:top]:
    print(f"{ca[k]:7} {cb[k]:7} {ca[k] - cb[k]:+7}  {k[0][:14]:14} {k[1]}")
