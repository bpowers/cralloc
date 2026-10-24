#!/usr/bin/env python3
"""Where a process's dTLB misses land: IBS op samples (user mode, with data addresses) sorted
by the asmalloc region their address falls in, by the map the switch writes (`WBH_MAP`, or the
`r` lines of `WBH_DIAG`), for docs/research/whole-block-huge.md.

  ibs.py record OUT.data PERIOD -- command...     (perf record of a command)
  ibs.py attach OUT.data PERIOD PID SECONDS        (perf record of a running process)
  ibs.py classify OUT.data MAP [--json]

A sample's TLB outcome is IBS's: an L1 hit, an L2 hit (an L1 miss), or an L2 miss (a page
walk). Buckets: small classes, large classes below 2 MiB, large classes of 2 MiB up to 16 MiB,
of 16 MiB or more, and memory outside the heap (stacks, the binary, other mappings).
"""
import json
import subprocess
import sys
from collections import Counter, defaultdict

EVENT = "ibs_op/cnt_ctl=1,swfilt=1/u"


def regions(path):
    out = []
    for line in open(path):
        f = line.split()
        if f and f[0] == "r":
            if len(f) == 6:  # WBH_MAP: r index base bytes size small|large
                _, _, base, nbytes, size, kind = f
            else:  # WBH_DIAG: r index base size small|large, with region_bytes in the header
                _, _, base, size, kind = f
                nbytes = 1 << 32
            out.append((int(base), int(base) + int(nbytes), int(size), kind))
    return sorted(set(out))


def bucket(size, kind):
    if kind == "small":
        return "small"
    if size < 2 << 20:
        return "large <2M"
    if size < 16 << 20:
        return "large 2-16M"
    return "large >=16M"


def classify(data, mapfile):
    rs = regions(mapfile)
    import bisect
    starts = [r[0] for r in rs]
    tally = defaultdict(Counter)
    p = subprocess.run(["perf", "script", "-i", data, "-F", "addr,data_src"], capture_output=True, text=True)
    for line in p.stdout.splitlines():
        f = line.split(None, 2)
        if len(f) < 3 or "TLB" not in f[2]:
            continue
        addr = int(f[0], 16)
        if addr == 0:
            continue
        tlb = f[2].split("TLB ", 1)[1].split("|", 1)[0].strip()
        i = bisect.bisect_right(starts, addr) - 1
        b = "outside heap"
        if i >= 0 and rs[i][0] <= addr < rs[i][1]:
            b = bucket(rs[i][2], rs[i][3])
        tally[b]["mem ops"] += 1
        if tlb in ("L2 hit", "L2 miss", "L2 hit or miss", "Walker hit", "Walker miss"):
            tally[b]["L1 miss"] += 1
        if tlb in ("L2 miss", "Walker miss", "Walker hit"):
            tally[b]["walk"] += 1
    return tally


def main():
    cmd = sys.argv[1]
    if cmd == "record":
        out, period = sys.argv[2], sys.argv[3]
        rest = sys.argv[sys.argv.index("--") + 1:]
        sys.exit(subprocess.run(["perf", "record", "-q", "-e", EVENT, "-c", period, "-d", "-o", out, "--"] + rest).returncode)
    if cmd == "attach":
        out, period, pid, secs = sys.argv[2:6]
        sys.exit(subprocess.run(["perf", "record", "-q", "-e", EVENT, "-c", period, "-d", "-o", out, "-p", pid,
                                 "--", "sleep", secs]).returncode)
    if cmd == "classify":
        t = classify(sys.argv[2], sys.argv[3])
        if "--json" in sys.argv:
            print(json.dumps({k: dict(v) for k, v in t.items()}))
            return
        tot = Counter()
        for v in t.values():
            tot.update(v)
        print(f"{'bucket':14} {'mem ops':>9} {'L1 miss':>8} {'walks':>7} {'% of walks':>10}")
        for b in ("small", "large <2M", "large 2-16M", "large >=16M", "outside heap"):
            v = t.get(b, Counter())
            share = 100 * v["walk"] / tot["walk"] if tot["walk"] else 0
            print(f"{b:14} {v['mem ops']:9d} {v['L1 miss']:8d} {v['walk']:7d} {share:9.1f}%")
        print(f"{'total':14} {tot['mem ops']:9d} {tot['L1 miss']:8d} {tot['walk']:7d}")


if __name__ == "__main__":
    main()
