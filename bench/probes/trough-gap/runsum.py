#!/usr/bin/env python3
"""One line per run of a directory of runs.sh output (docs/research/trough-gap.md): median
trough heap (GiB), median seconds for a trough to come within a tenth of its end, peak ratio,
heap after the quiet minute, cycles an operation, faults a thousand operations, and the
diagnostic build's MADV_DONTNEED and split calls and bytes returned by the clocked pass.

    python3 -I runsum.py DIR...
"""
import json
import statistics
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
SUMMARY = HERE.parent / "large-heap-constants/summary.py"


def main():
    for d in sys.argv[1:]:
        out = subprocess.run([sys.executable, "-I", str(SUMMARY), d, "--diag"], capture_output=True, text=True).stdout
        row, extra = None, {}
        rows = []
        for line in out.splitlines():
            s = line.strip()
            if s.startswith('{"job"'):
                if row:
                    rows.append((row, extra))
                row, extra = json.loads(s), {}
            elif s.startswith("dontneed ") or s.startswith("split "):
                k, v = s.split(" ", 1)
                extra[k] = json.loads(v)
            elif s.startswith('{"passes"'):
                extra["sc"] = json.loads(s)
        if row:
            rows.append((row, extra))
        print(f"== {d}")
        for r, e in rows:
            med = lambda k: statistics.median(r[k]) if r.get(k) else float("nan")
            settle = [x for x in r.get("trough_settle_s", []) if x is not None]
            print(f"{r['build']:>10} trough {med('trough_heap_gib'):.3f} GiB settle {statistics.median(settle) if settle else float('nan'):.1f}s "
                  f"peak {med('peaks'):.3f} quiet {r.get('quiet_heap_gib', float('nan'))} "
                  f"cyc/op {r.get('cycles_op')} flt/kop {r.get('faults_per_kop')} "
                  f"dontneed {e.get('dontneed', {}).get('n')} split {e.get('split', {}).get('n')} "
                  f"returned {round(e.get('sc', {}).get('decay_returned_other', 0) / 2**30, 2)} GiB phase {r.get('phase_s')}s ok {r['ok']}")


if __name__ == "__main__":
    main()
