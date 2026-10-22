"""meilisum.py RESULTS.jsonl...: per allocator and run, Meilisearch's cycles/req, p99.9, faults per
request at 70%, its heap (anonymous memory, MiB) at the end of each ten-second piece of the minute
at 70%, the swing between those, and 9.75 s after the load stops; and the CREDITDIAG line of
diagnostic builds, in MiB."""
import json
import re
import sys
from collections import defaultdict

runs = defaultdict(list)
for f in sys.argv[1:]:
    for line in open(f):
        d = json.loads(line)
        if d.get("type") == "run" and d.get("served"):
            runs[d["allocator"]].append(d)


def heap_at(served, t):
    v = [p[2] for p in served["curve_kib"] if p[0] <= t]
    return v[-1] / 1024 if v else float("nan")


MIB = ("carve_small", "carve_large", "refault_large", "refault_small", "owed", "paid1", "paid2",
       "dropped", "hold", "decay", "avoidable", "avoidable_whole", "cover")

for a, rs in sorted(runs.items()):
    for r in rs:
        s = r["served"]
        g = s["groups"]["s70"]
        s70 = [m for m in s["marks_ms"] if m[1] == "s70"]
        idle = [m for m in s["marks_ms"] if m[1] == "idle"]
        pieces = [heap_at(s, m[0] + 9900) for m in s70]
        rest = heap_at(s, idle[0][0] + 9750) if idle else float("nan")
        print(f"{a:6s} r{r['round']} valid {int(bool(r.get('valid')))} cyc {r['primary']['value'] / 1e6:6.2f}M"
              f" p999 {g['lat_us']['p999'] / 1000:5.1f}ms faults/req {g['server']['faults_per_req']:6.1f}"
              f" heap {' '.join(f'{x:4.0f}' for x in pieces)} swing {max(pieces) - min(pieces):4.0f} rest {rest:4.0f}")
        tail = r.get("stderr_tail") or ""
        m = re.findall(r"CREDITDIAG[^\n]*", tail)
        if m:
            kv = m[-1].split()[1:]
            d = dict(zip(kv[::2], (int(x) for x in kv[1::2])))
            out = " ".join(f"{k} {d[k] / 1048576:.0f}" if k in MIB else f"{k} {d[k]}" for k in d)
            print("        ", out)
        if r.get("problems"):
            print("         problems:", r["problems"])
