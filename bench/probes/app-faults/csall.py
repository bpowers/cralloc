#!/usr/bin/env python3
"""For each scratch/cs/PREFIX-*.data: wall, switch count and the share of switches by reason."""
import os
import collections
import subprocess
import sys
from pathlib import Path

D = Path(os.environ.get("APPF_OUT", "/tmp/app-faults") + "/cs")
REASONS = [("fault-mmap-read", ["rwsem_down_read_slowpath", "do_user_addr_fault"]),
           ("mprotect-mmap-write", ["rwsem_down_write_slowpath", "__x64_sys_mprotect"]),
           ("mprotect-vma-write", ["__vma_start_write", "__x64_sys_mprotect"]),
           ("madvise-mmap", ["rwsem_down", "__x64_sys_madvise"]),
           ("madvise-other", ["__x64_sys_madvise"]),
           ("mprotect-other", ["__x64_sys_mprotect"]),
           ("futex", ["futex_wait"]),
           ("preempt", ["irqentry_exit"]),
           ("yield", ["sched_yield"]),
           ("sleep", ["do_nanosleep"]),
           ("fault-other", ["do_user_addr_fault"])]


def classify(syms):
    for name, keys in REASONS:
        if all(any(k in s for s in syms) for k in keys):
            return name
    return "other"


for prefix in sys.argv[1:]:
    rows = []
    for data in sorted(D.glob(f"{prefix}-*.data"), key=lambda p: int(p.stem.rsplit("-", 1)[1])):
        out = subprocess.run(["perf", "script", "-i", str(data), "-F", "ip,sym"], capture_output=True, text=True).stdout
        counts = collections.Counter()
        for block in out.split("\n\n"):
            syms = [l.strip() for l in block.splitlines() if l.strip()]
            if syms:
                counts[classify(syms)] += 1
        wall = float(data.with_suffix(".wall").read_text())
        rows.append((wall, sum(counts.values()), counts))
    names = [n for n, _ in REASONS] + ["other"]
    print(f"{'run':10} {'wall':>6} {'cs':>5} " + " ".join(f"{n[:12]:>12}" for n in names))
    for i, (wall, tot, c) in enumerate(rows):
        print(f"{prefix}-{i + 1:<4} {wall:6.3f} {tot:5d} " + " ".join(f"{c[n]:12d}" for n in names))
