#!/usr/bin/env python3
"""IBS op samples in the allocator's code by the innermost inlined function and by whether the
address lies inside a pop or push routine's range (arming included): how many ops each part of
the inlined fast path costs a request. IBS tags ops precisely, so this does not suffer the skid
that cycles samples do; ops are not cycles.

  opsby.py DIR      (a record.py --mode ibs directory of an asmalloc run)
"""
import collections
import json
import os
import subprocess
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import elfinfo
from where import RATES, Symbolizer, asm_path, load_base

d = sys.argv[1]
meta = json.load(open(f"{d}/meta.json"))
binary = meta["server"]
base = load_base(open(f"{d}/maps.txt").read(), binary)
reqs = RATES[meta["job"]] * meta["dur"]
period = meta["period"]
rng = elfinfo.Ranges(binary)
out = subprocess.run(["perf", "script", "-i", f"{d}/perf.data", "-F", "pid,ip", "--no-inline"],
                     capture_output=True, text=True).stdout
ips = []
for line in out.splitlines():
    f = line.split()
    if len(f) == 2 and int(f[0]) == meta["server_pid"]:
        ips.append(int(f[1], 16) - base)
sym = Symbolizer(binary)
sym.resolve([a for a in ips if 0 < a < 1 << 40])
c = collections.Counter()
for a in ips:
    ch = sym[a] if 0 < a < 1 << 40 else []
    path = asm_path(ch) if ch else None
    if not path:
        continue
    k = rng.kind(a) or "outside routines"
    c[(path, k, ch[0][0].split("(")[0][-50:])] += 1
for (path, k, fn), n in c.most_common(25):
    print(f"{n * period / reqs:9.0f} ops/req  {path:16} {k:17} {fn}")
by = collections.Counter()
for (path, k, fn), n in c.items():
    by[(path, k)] += n
print()
for (path, k), n in by.most_common():
    print(f"{n * period / reqs:9.0f} ops/req  {path:16} {k}")
