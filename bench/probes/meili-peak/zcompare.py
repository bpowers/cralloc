#!/usr/bin/env python3
"""zbench outputs of two modes, interleaved runs, row against row: the median over runs of each
row's kcycles, and the rule's ratio to main's (docs/research/meili-peak.md).

  zcompare.py --main FILE... --rule FILE...
"""
import statistics
import sys


def rows(path):
    out = {}
    for line in open(path):
        f = line.split()
        if len(f) == 5 and f[0].isdigit():
            out[(int(f[0]), f[1], f[2])] = (float(f[3]), float(f[4]))
    return out


def main():
    args = sys.argv[1:]
    i = args.index("--rule")
    mains, rules = [rows(p) for p in args[1:i]], [rows(p) for p in args[i + 1:]]
    print(f"{'MiB':>4} {'slot':>9} {'frac':>5} {'main kcyc':>10} {'rule kcyc':>10} {'ratio':>6} {'faults m/r':>11}")
    for key in mains[0]:
        m = statistics.median(r[key][0] for r in mains)
        r = statistics.median(x[key][0] for x in rules)
        fm, fr = mains[0][key][1], rules[0][key][1]
        print(f"{key[0]:>4} {key[1]:>9} {key[2]:>5} {m:>10.1f} {r:>10.1f} {r / m:>6.2f} {fm:>5.0f}/{fr:<5.0f}")


if __name__ == "__main__":
    main()
