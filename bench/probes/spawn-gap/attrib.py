#!/usr/bin/env python3
"""Attribute IBS load samples (perf script text) to the heap blocks of an allocation trace.

usage: attrib.py SAMPLES.txt TRACE [N]
SAMPLES.txt from: perf script -F cpu,addr,weight,data_src,ip,sym
Prints, for loads that missed L1, counts by (block layout, offset line within block, level),
and by symbol.
"""
import bisect, collections, re, sys
sys.path.insert(0, __import__("os").path.dirname(__import__("os").path.abspath(__file__)))
from trace import load

samples, tracef = sys.argv[1], sys.argv[2]
top = int(sys.argv[3]) if len(sys.argv) > 3 else 40
recs = load(tracef)
at = collections.defaultdict(collections.Counter)
first_index = {}
for i, r in enumerate(recs):
    if r[1] in (0, 2) and r[5]:
        at[r[5]][(r[4], 1 << r[3])] += 1
        first_index.setdefault(r[5], i)
ptrs = sorted(at)
maxsize = {p: max(s for s, a in at[p]) for p in ptrs}

def block(x):
    i = bisect.bisect_right(ptrs, x) - 1
    if i < 0:
        return None
    p = ptrs[i]
    if x < p + maxsize[p]:
        (s, a), _ = at[p].most_common(1)[0]
        return p, s, a
    return None

lv = collections.Counter()
by_obj = collections.Counter()
by_obj_w = collections.Counter()
by_sym = collections.Counter()
nload = 0
bad = 0
def parse(line):
    parts = line.rstrip("\n").split("|")
    if len(parts) < 7:
        return None
    head = parts[0].split()
    cpu, addr = head[0], head[1]
    op = parts[1].replace("OP ", "").strip()
    lvl = parts[2].replace("LVL ", "").strip()
    snp = parts[3].replace("SNP ", "").strip()
    tail = parts[6].split()
    # tail: BLK [N/A] weight ip sym...
    i = 1
    if i < len(tail) and tail[i] == "N/A":
        i += 1
    lat = tail[i] if i < len(tail) else "0"
    ip = tail[i + 1] if i + 1 < len(tail) else "0"
    sym = " ".join(tail[i + 2:])
    return cpu, addr, op, lvl, snp, lat, ip, sym
for line in open(samples):
    g = parse(line)
    if not g:
        bad += 1
        continue
    cpu, addr, op, lvl, snp, lat, ip, sym = g
    if op != "LOAD":
        continue
    addr = int(addr, 16)
    if addr >= 0x7FFF_FFFF_FFFF:
        continue  # kernel
    nload += 1
    if lvl.startswith("L1"):
        continue
    level = "HITM" if "HitM" in snp else ("L3" if "L3" in lvl else ("L2" if "L2" in lvl else lvl.split()[0]))
    lv[level] += 1
    b = block(addr)
    if b:
        p, s, a = b
        key = (f"size {s} align {a}", (addr - p) // 64 * 64)
    else:
        key = ("not heap", (addr >> 12) << 12)
    by_obj[(key, level)] += 1
    by_obj_w[key] += int(lat or 0)
    by_sym[(sym.split("(")[0][:110], level)] += 1
print("user loads", nload, "unparsed", bad, "L1 misses by level", dict(lv))
print("\nby block and line (L1-missing loads):")
agg = collections.defaultdict(collections.Counter)
for (key, level), n in by_obj.items():
    agg[key][level] += n
for key, c in sorted(agg.items(), key=lambda kv: -sum(kv[1].values()))[:top]:
    print(f"  {key[0]:24} +{key[1]:#6x}  total {sum(c.values()):5}  " + " ".join(f"{k}={v}" for k, v in sorted(c.items())) + f"  latency_sum {by_obj_w[key]}")
print("\nby layout:")
agg2 = collections.defaultdict(collections.Counter)
for (key, level), n in by_obj.items():
    agg2[key[0]][level] += n
for k, c in sorted(agg2.items(), key=lambda kv: -sum(kv[1].values()))[:20]:
    print(f"  {k:24} total {sum(c.values()):5}  " + " ".join(f"{a}={b}" for a, b in sorted(c.items())))
print("\nby symbol:")
agg3 = collections.defaultdict(collections.Counter)
for (s, level), n in by_sym.items():
    agg3[s][level] += n
for k, c in sorted(agg3.items(), key=lambda kv: -sum(kv[1].values()))[:25]:
    print(f"  {sum(c.values()):5} " + " ".join(f"{a}={b}" for a, b in sorted(c.items())) + "  " + k)
