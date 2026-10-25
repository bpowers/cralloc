#!/usr/bin/env python3
"""Huge pages a server session's processes held, from thpwatch.sh's samples, by setting:
the mean and largest resident and AnonHugePages over the samples in which the process held
at least half its largest resident (the loaded part of the run), and the last sample.

    python3 -I bench/probes/huge-tail/thpsumm.py FILE.thp [FILE.ht]

With the switch's counter file, also its last line per process (a run starts the epoch over).
"""
import re
import sys
from collections import defaultdict


def main():
    runs = defaultdict(list)
    for line in open(sys.argv[1]):
        f = line.split()
        if len(f) != 5:
            if "free2m=" in line:
                print("free 2 MiB blocks at start:", line.split("free2m=")[1].strip())
            continue
        ms, ht, exe, rss, huge = int(f[0]), f[1], f[2], int(f[3]), int(f[4])
        runs[(ht, exe)].append((ms, rss, huge))
    print("| setting | program | samples | loaded mean rss MiB | loaded mean huge MiB | huge share | last rss | last huge |")
    print("|---|---|---|---|---|---|---|---|")
    for (ht, exe), xs in runs.items():
        top = max(r for _, r, _ in xs)
        loaded = [x for x in xs if x[1] >= top / 2]
        mr = sum(x[1] for x in loaded) / len(loaded) / 1024
        mh = sum(x[2] for x in loaded) / len(loaded) / 1024
        print(f"| {ht} | {exe} | {len(xs)} | {mr:.0f} | {mh:.0f} | {mh / mr:.0%} | {xs[-1][1] / 1024:.0f} | {xs[-1][2] / 1024:.0f} |")
    if len(sys.argv) > 2:
        last, prev = [], None
        for line in open(sys.argv[2]):
            e = int(re.search(r"epoch=(\d+)", line).group(1))
            if prev is not None and e < prev[0]:
                last.append(prev[1])
            prev = (e, line.strip())
        if prev:
            last.append(prev[1])
        for l in last:
            print(" ", l[:300])


if __name__ == "__main__":
    main()
