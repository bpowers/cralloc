"""Median memory per job and build from acct.sh output.

usage: peaks.py DIR [label...]

Two tables, MiB, medians over rounds:

- the cgroup peak: the one a memacct build recorded before its own buffers were touched
  (PREFIX.acct), else the cgroup's peak after exit (PREFIX.peak);
- what the process was charged at exit for its own pages, its page tables and kernel slab
  objects, from a flushed memory.stat. This leaves out the per-CPU charge stock (up to 64 pages
  per CPU that charged), which makes the peak of a cgroup this small move by a few hundred KiB
  from run to run.
"""
import collections
import pathlib
import re
import statistics
import sys

out = pathlib.Path(sys.argv[1])
want = sys.argv[2:]
peaks = collections.defaultdict(list)
charged = collections.defaultdict(list)
for f in sorted(out.glob('*.peak')):
    job, label, _ = f.name[:-len('.peak')].rsplit('.', 2)
    if want and label not in want:
        continue
    acct = f.with_suffix('.acct')
    peak = None
    if acct.exists():
        text = acct.read_text(errors='replace')
        m = re.search(r'^cgroup current=\d+ peak=(\d+)', text, re.M)
        peak = int(m[1]) if m else None
        stat = dict(re.findall(r'^stat (\w+)=(\d+)', text, re.M))
        if int(stat.get('anon', 0)):
            charged[(job, label)].append(sum(int(stat.get(k, 0)) for k in ('anon', 'pagetables', 'slab')) / 2**20)
    if peak is None:
        try:
            peak = int(f.read_text().split()[0])
        except (ValueError, IndexError):
            continue
    peaks[(job, label)].append(peak / 2**20)
jobs = sorted({j for j, _ in peaks})
labels = want or sorted({l for _, l in peaks})
for title, table in (('cgroup peak', peaks), ('charged at exit: anonymous + page tables + kernel slab', charged)):
    print(f'{title}, MiB, median')
    print(f'{"job":10}' + ''.join(f'{l:>14}' for l in labels))
    for j in jobs:
        cells = [f'{statistics.median(table[(j, l)]):14.2f}' if table.get((j, l)) else f'{"-":>14}' for l in labels]
        print(f'{j:10}' + ''.join(cells))
