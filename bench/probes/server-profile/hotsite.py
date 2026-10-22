#!/usr/bin/env python3
"""The addresses in the server binary where IBS ops land in one inlined function of the
allocator's (by name), most sampled first, to disassemble the commonest call site.

  hotsite.py DIR NAME [N]
"""
import collections
import json
import os
import subprocess
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from where import Symbolizer, load_base

d, name = sys.argv[1], sys.argv[2]
n = int(sys.argv[3]) if len(sys.argv) > 3 else 10
meta = json.load(open(f"{d}/meta.json"))
binary = meta["server"]
base = load_base(open(f"{d}/maps.txt").read(), binary)
out = subprocess.run(["perf", "script", "-i", f"{d}/perf.data", "-F", "pid,ip", "--no-inline"],
                     capture_output=True, text=True).stdout
c = collections.Counter()
for line in out.splitlines():
    f = line.split()
    if len(f) == 2 and int(f[0]) == meta["server_pid"]:
        a = int(f[1], 16) - base
        if 0 < a < 1 << 40:
            c[a] += 1
sym = Symbolizer(binary)
sym.resolve(list(c))
hits = collections.Counter()
for a, k in c.items():
    if sym[a] and name in sym[a][0][0]:
        hits[a] += k
for a, k in hits.most_common(n):
    print(f"{a:#x} {k}  {' < '.join(fn.split('(')[0][-40:] for fn, _ in sym[a][:4])}")
