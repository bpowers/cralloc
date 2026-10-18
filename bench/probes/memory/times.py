"""Primary metric per job and build from acct.sh's printed lines, against a reference build.

usage: times.py FILE REF [label...]

Prints the median of each build and, for every other build, the median of its per-round ratios
to REF as a change in percent, with the lowest and highest of those ratios.
"""
import collections
import re
import statistics
import sys

path, ref = sys.argv[1], sys.argv[2]
want = sys.argv[3:]
values = collections.defaultdict(dict)
for line in open(path):
    m = re.match(r'(\S+)\s+(\S+)\s+round=(\d+)\s+cgroup_peak_MiB=\S*\s+primary=(\S+)', line)
    if m:
        values[(m[1], m[2])][int(m[3])] = float(m[4])
jobs = sorted({j for j, _ in values})
labels = want or sorted({l for _, l in values})
print(f'{"job":10}' + ''.join(f'{l:>12}' for l in labels) + ''.join(f'{l + " vs " + ref:>28}' for l in labels if l != ref))
for j in jobs:
    row = f'{j:10}'
    for l in labels:
        v = values.get((j, l), {})
        row += f'{statistics.median(v.values()):12.4g}' if v else f'{"-":>12}'
    base = values.get((j, ref), {})
    for l in labels:
        if l == ref:
            continue
        v = values.get((j, l), {})
        ratios = [v[r] / base[r] for r in v if r in base]
        if ratios:
            med = (statistics.median(ratios) - 1) * 100
            row += f'{med:+9.1f}% [{(min(ratios) - 1) * 100:+.1f}, {(max(ratios) - 1) * 100:+.1f}]'.rjust(28)
        else:
            row += f'{"-":>28}'
    print(row)
