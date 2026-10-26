#!/usr/bin/env python3
"""What the allocator did in the first milliseconds of each burst after a silence, from an
`HT_TRACE` file (ht.rs): for every heartbeat that came after more than SILENCE seconds without
one, the walk calls (`W`), raises (`R`) and lowerings (`L`) in the WINDOW ms after it, with
who ran them (the background thread or a request's slow path), on which CPU, how long each
took and what it read and returned.

    python3 -I bench/probes/huge-tail/burst.py FILE.trace [--silence 10] [--window 20]

A trace line: kind t0_ns dur_ns tid bg cpu a b c d e. For `W`: a region, b scope<<8 |
counting<<1 | zone, c words read, d pages returned, e splits. For `H`: a the gap in ns, b 1 at
a burst's first heartbeat.
"""
import sys


def main():
    args = sys.argv[1:]
    path = args[0]
    silence = float(args[args.index("--silence") + 1]) if "--silence" in args else 10.0
    window = float(args[args.index("--window") + 1]) if "--window" in args else 20.0
    rows = []
    for line in open(path):
        f = line.split()
        if len(f) != 11:
            continue
        rows.append((f[0], *map(int, f[1:])))
    rows.sort(key=lambda r: r[1])
    starts = [r[1] for r in rows if r[0] == "H" and r[7] == 1 and r[6] > silence * 1e9]
    for s in starts:
        inside = [r for r in rows if s <= r[1] < s + window * 1e6 and r[0] != "H"]
        hb = [r for r in rows if s <= r[1] < s + window * 1e6 and r[0] == "H"]
        walks = [r for r in inside if r[0] == "W"]
        busy = sum(r[2] for r in walks)
        print(f"burst at {s}: {len(hb)} heartbeats, {len(walks)} walk calls ({busy / 1e3:.0f} us), "
              f"{sum(1 for r in inside if r[0] == 'R')} raises, {sum(1 for r in inside if r[0] == 'L')} lowerings in {window:.0f} ms")
        for r in inside:
            kind, t0, dur, tid, bg, cpu, a, b, c, d, e = r
            who = "thread" if bg else f"tid {tid}"
            if kind == "W":
                print(f"  +{(t0 - s) / 1e3:8.0f} us W {who:10s} cpu {cpu:2d} {dur / 1e3:7.1f} us region {a} zone {b & 1} counting {(b >> 1) & 1} scope {b >> 8} read {c} returned {d} splits {e}")
            else:
                print(f"  +{(t0 - s) / 1e3:8.0f} us {kind} {who:10s} cpu {cpu:2d} region {a} {b}..{c}")


if __name__ == "__main__":
    main()
