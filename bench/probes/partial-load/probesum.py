"""Per-request slow-path counts from partial.py probe runs (crates/asmalloc/src/probe.rs)."""
import json
import sys
from pathlib import Path

SMALL = None  # set from the table size: CPUS * SMALL_COUNT * KINDS
KINDS = 16
CPUS = 32
NAMES = ["AllocSlow", "FreeSlow", "AllocSpare", "AllocDepot", "AllocFill", "EmptySpare", "EmptyDepot",
         "EmptyMade", "RetFullSpare", "RetFullDepot", "RetEmptySpare", "RetEmptyDepot", "StockAdmit", "Launder",
         "AllocInstLvl", "FreeDispLvl"]
small = int(sys.argv[1])  # SMALL_COUNT
for d in sys.argv[2:]:
    for p in sorted(Path(d).glob("*.probe.json")):
        tag = p.name[:-11]
        n = json.loads((p.parent / f"{tag}.result.json").read_text())["groups"]["m"]["lat_us"]["n"]
        delta = {int(k): v for k, v in json.loads(p.read_text()).items()}
        tot = [0] * KINDS
        pairs = {}
        for i, v in delta.items():
            kind = i % KINDS
            cls = (i // KINDS) % small
            cpu = i // KINDS // small
            tot[kind] += v
            if kind in (0, 1):
                pairs[(cpu, cls)] = pairs.get((cpu, cls), 0) + v
        per = {NAMES[k]: round(tot[k] / n, 3) for k in range(14) if tot[k]}
        top = sorted(pairs.items(), key=lambda kv: -kv[1])[:8]
        print(f"{d.split('/')[-1]}/{tag}: per request {per}")
        print("   top (cpu, class ordinal): slow paths per request", [(k, round(v / n, 3)) for k, v in top])
