#!/usr/bin/env python3
"""A machine's fixed rates, from sessions of `servers-sat` and `servers-large-sat`:
rates.py FILE... > bench/rates/<implementer>-<part>.toml

For each server job, 70% of the slowest allocator's saturated rate, the median over its valid runs,
to three figures. `benchrun` reads the file (suite.rs, `machine_rates`). Pass it only the allocators
a job should be sized by: the Router's `fast` is its plain job's without glibc."""
import collections
import json
import statistics
import sys

rates = collections.defaultdict(lambda: collections.defaultdict(list))
for path in sys.argv[1:]:
    for line in open(path):
        r = json.loads(line)
        if r.get("type") != "run" or not r.get("valid") or not r["job"].endswith(" sat"):
            continue
        sat = r["served"]["groups"]["sat"]
        rates[r["job"].removesuffix(" sat")][r["allocator"]].append(sat["rate"])
for job in sorted(rates):
    per = {a: statistics.median(v) for a, v in rates[job].items()}
    slowest = min(per, key=per.get)
    r70 = per[slowest] * 0.7
    digits = max(0, len(str(int(r70))) - 3)
    rounded = round(r70, -digits)
    others = ", ".join(f"{a} {per[a]:.0f}" for a in sorted(per, key=per.get))
    print(f"# {others}")
    print(f'"{job}" = {int(rounded)}')
