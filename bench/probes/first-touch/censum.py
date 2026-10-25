#!/usr/bin/env python3
"""Counts a census.sh strace by call (madvise by advice), over the window it covers: calls a
second, and the median and largest time in the call. Calls that take the address space's lock
for writing are marked W. Under strace the times are inflated; the counts are the point.

  censum.py FILE.strace
"""
import re
import sys
from collections import defaultdict

LINE = re.compile(r"^(\d+)\s+([\d.]+)\s+(\w+)\((.*)\)\s+=\s+(\S+).*<([\d.]+)>$")
WRITERS = {"mmap", "munmap", "mprotect", "mremap", "brk", "MADV_HUGEPAGE", "MADV_NOHUGEPAGE",
           "mlock", "munlock"}


def main():
    calls = defaultdict(list)
    t = []
    for line in open(sys.argv[1]):
        m = LINE.match(line.strip())
        if not m:
            continue
        tid, ts, name, args, ret, dur = m.groups()
        if name == "madvise":
            name = args.split(",")[-1].strip()
        calls[name].append((float(dur) * 1e6, int(tid)))
        t.append(float(ts))
    if not t:
        print("no calls")
        return
    span = max(max(t) - min(t), 1e-3)
    print(f"window {span:.1f} s")
    for name, v in sorted(calls.items(), key=lambda kv: -len(kv[1])):
        d = sorted(x[0] for x in v)
        w = "W" if name in WRITERS else " "
        print(f"  {w} {name:18} {len(v):6} calls, {len(v) / span:8.1f}/s, threads {len(set(x[1] for x in v)):3}, "
              f"median {d[len(d) // 2]:7.1f} us, max {d[-1]:8.1f} us")


if __name__ == "__main__":
    main()
