#!/usr/bin/env python3
"""Where the empty pages that the purge cannot return lie, against each region's aged
frontier, from the unit maps the huge-tail memstat build writes (ht_edit.py).

    python3 -I bench/probes/huge-tail/where.py FILE.memstat [--regions]

For each breakdown: the resident 4 KiB pages wholly free inside 2 MiB units of a huge-page
part that hold something (what "Never split a huge page" keeps), split by whether the unit
starts at or above the region's frontier rounded up to a unit (what the tail rule reaches) or
below it. With --regions, a line per region holding more than 4 MiB of them.
"""
import re
import sys
from pathlib import Path

KV = re.compile(r"(\w+)=(\S+)")


def main():
    args = sys.argv[1:]
    if not args:
        sys.exit(__doc__)
    per_region = "--regions" in args
    tag = None
    rows = {}
    order = []
    for line in Path(args[0]).read_text().splitlines():
        if line.startswith("account tag="):
            tag = int(line.split("=")[1])
            continue
        if not line.startswith("acct-units "):
            continue
        kv = dict(KV.findall(line))
        r = rows.setdefault(tag, [])
        if tag not in order:
            order.append(tag)
        r.append(kv)
    print("| breakdown | regions | stuck above frontier MiB | stuck below MiB | share above | huge part carved above frontier MiB |")
    print("|---|---|---|---|---|---|")
    for t in order:
        r = rows[t]
        above = sum(int(k["stuck_above_kib"]) for k in r) / 1024
        below = sum(int(k["stuck_below_kib"]) for k in r) / 1024
        span = sum(max(0, int(k["carved_mib"]) - max(int(k["frontier_mib"]), int(k["huge_from_mib"]))) for k in r)
        share = above / (above + below) if above + below else 0
        print(f"| {t} | {len(r)} | {above:.0f} | {below:.0f} | {share:.0%} | {span} |")
        if per_region:
            for k in r:
                a, b = int(k["stuck_above_kib"]) / 1024, int(k["stuck_below_kib"]) / 1024
                if a + b > 4:
                    m = k["map"]
                    print(
                        f"    region {k['region']} size {k['size']}: carved {k['carved_mib']} MiB, huge from {k['huge_from_mib']}, "
                        f"frontier {k['frontier_mib']}, tail {k['tail_mib']}; stuck above {a:.0f} below {b:.0f} MiB; "
                        f"map {m[:160]}{'...' if len(m) > 160 else ''}"
                    )


if __name__ == "__main__":
    main()
