#!/usr/bin/env python3
"""Peaks, troughs and the quiet minute of drift `lifetime` runs (docs/research/trough-gap.md).

    python3 -I phases.py FILE...

For each run file (drift.py's output: DRIFT lines), one line per phase end: the phase, resident
heap over live, heap and live in GiB, huge pages (AnonHugePages) and the cgroup's charge in GiB,
seconds since the start; then the end of the quiet minute. Then the medians over the run's
troughs and peaks, as the earlier notes report them.
"""
import json
import statistics
import sys
from pathlib import Path

GIB = 1 << 30


def load(path):
    rows = []
    for line in Path(path).read_text().splitlines():
        if line.startswith("DRIFT "):
            rows.append(json.loads(line[6:]))
    return rows


def ends(rows):
    """The last load sample of each phase, in order, and the last quiet sample."""
    out, last = [], None
    load_rows = [r for r in rows if r["kind"] == "load"]
    for i, r in enumerate(load_rows):
        nxt = load_rows[i + 1] if i + 1 < len(load_rows) else None
        if nxt is None or nxt["phase"] != r["phase"]:
            out.append(r)
    quiet = [r for r in rows if r["kind"] == "quiet"]
    return out, (quiet[-1] if quiet else None)


def fmt(r):
    return (f"{r['phase']:>8} ratio {r['ratio']:.3f} heap {r['heap'] / GIB:.3f} live {r['live'] / GIB:.3f} "
            f"thp {r['thp'] / GIB:.3f} cgroup {r['cgroup'] / GIB:.3f} t {r['t_ms'] / 1000:.0f}s")


def main():
    for path in sys.argv[1:]:
        rows = load(path)
        if not rows:
            print(f"{path}: no samples")
            continue
        phase_ends, quiet = ends(rows)
        print(f"== {path} ({rows[0]['alloc']})")
        for r in phase_ends:
            print("  " + fmt(r))
        if quiet:
            print("  " + fmt(quiet).replace("   quiet", "   quiet"))
        lo = [r for r in phase_ends if r["phase"].startswith("lo")]
        hi = [r for r in phase_ends if r["phase"].startswith("hi")]
        if lo and hi:
            med = lambda xs, k: statistics.median(x[k] for x in xs)
            print(f"  median peaks {med(hi, 'ratio'):.3f}; troughs {med(lo, 'ratio'):.3f} "
                  f"(heap {med(lo, 'heap') / GIB:.3f} GiB, live {med(lo, 'live') / GIB:.3f}); "
                  f"quiet {quiet['ratio'] if quiet else float('nan'):.3f} "
                  f"(heap {quiet['heap'] / GIB if quiet else float('nan'):.3f})")


if __name__ == "__main__":
    main()
