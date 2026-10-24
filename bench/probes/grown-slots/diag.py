#!/usr/bin/env python3
"""Summarizes the grown-slots counters (gs.rs) in kern.py's or bench.py's records.

  diag.py RUNS.jsonl [--job SUBSTR] [--arm NAME]

For each run with a diag line: moves, what the rule kept and refused, what was returned by
each rule, and the clean bytes later claims and growth in place took back, by what had last
returned them and how long before. MiB.
"""
import argparse
import json

TAGS = ["fresh", "move", "grownfree", "unwritten", "shrink", "hold", "pass", "growth", "trim"]
AGES = ["<32ms", "<64ms", "<125ms", "<250ms", "<.5s", "<1s", "<8s", "older"]
M = 2**20


def parse(line):
    d = {}
    for f in line.split()[1:]:
        k, v = f.split("=")
        d[int(k) if k.isdigit() else k] = int(v)
    return d


def show(d):
    g = lambda i: d.get(i, 0)
    print(f"  moves {g(0)} ({g(1)/M:.0f} MiB copied); kept move {g(2)} / {g(3)/M:.1f} MiB, refused {g(4)} / {g(5)/M:.1f}; "
          f"kept grown free {g(6)} / {g(7)/M:.1f}, refused {g(8)} / {g(9)/M:.1f}")
    print("  returned: " + ", ".join(f"{TAGS[t]} {g(10+t)/M:.1f}" for t in range(9) if g(10 + t)) + f"; small {g(36)/M:.1f}")
    print(f"  carved large {g(20)/M:.1f}, small {g(21)/M:.1f}; small revived {g(22)/M:.1f}; large claims {g(23)} "
          f"({g(24)/M:.0f} MiB); frees {g(25)} charged {g(26)/M:.0f}")
    print(f"  kept slots reused {g(29)} / {g(30)/M:.1f} MiB, purged {g(31)/M:.1f}; kept max {g(32)/M:.1f}, now {d.get('kept_now',0)/M:.1f}; "
          f"retained large max {g(33)/M:.1f}; heap max {g(34)/M:.0f}; budget max {g(35)/M:.0f}; live large max {g(37)/M:.0f}")
    if g(27):
        print(f"  mincore calls {g(27)}, marked clean {g(28)/M:.1f}")
    for base, what in ((64, "claims"), (160, "growth in place")):
        rows = []
        for t in range(9):
            vals = [g(base + t * 8 + a) for a in range(8)]
            if sum(vals):
                rows.append(f"    {TAGS[t]:10} {sum(vals)/M:8.1f}  " + " ".join(f"{AGES[a]}:{v/M:.1f}" for a, v in enumerate(vals) if v))
        if rows:
            print(f"  clean bytes taken back by {what}, by last return:")
            print("\n".join(rows))


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("runs")
    ap.add_argument("--job", default="")
    ap.add_argument("--arm", default="")
    a = ap.parse_args()
    for line in open(a.runs):
        if line.startswith("gs "):
            # A raw dump (GS_DIAG of a benchrun entry): one line a process.
            print(" ".join(line.split()[:6]))
            show(parse(line))
            continue
        r = json.loads(line)
        name = r.get("job") or r.get("prog")
        arm = r.get("arm") or r.get("lib")
        if a.job not in name or (a.arm and a.arm != arm) or "diag" not in r:
            continue
        print(f"{name} [{arm}] round {r.get('round')}")
        for dl in r["diag"].splitlines():
            show(parse(dl))


if __name__ == "__main__":
    main()
