"""Sums partial.py's per-thread perf stat files per run, per request of the measured group."""
import json
import sys
from pathlib import Path

d = Path(sys.argv[1])
for st in sorted(d.glob("*.stat")):
    tag = st.name[:-5]
    res = json.loads((d / f"{tag}.result.json").read_text())
    g = res["groups"]["m"]
    n = g["lat_us"]["n"]
    tot, per_thread = {}, {}
    for line in st.read_text().splitlines():
        f = line.split(",")
        if len(f) < 4 or line.startswith("#") or f[1].startswith("<"):
            continue
        thread, ev = f[0], f[3]
        v = float(f[1])
        tot[ev] = tot.get(ev, 0) + v
        per_thread.setdefault(thread.rsplit("-", 1)[0], {}).setdefault(ev, 0)
        per_thread[thread.rsplit("-", 1)[0]][ev] += v
    pr = lambda e: tot.get(e, 0) / n
    print(f"{tag:22s} n={n} rate={g['rate']:.0f} missed={g['missed']} p999={g['lat_us']['p999']:.0f}"
          f"  cyc/req u={pr('cycles:u'):.0f} k={pr('cycles:k'):.0f}  ins/req u={pr('instructions:u'):.0f}"
          f" k={pr('instructions:k'):.0f}  cpu_us/req={pr('task-clock') * 1e3:.2f}  cs/req={pr('context-switches'):.3f}"
          f"  migr/req={pr('cpu-migrations'):.4f} faults/req={pr('page-faults'):.4f}  threads={sorted(per_thread)}")
