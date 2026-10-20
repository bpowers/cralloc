#!/usr/bin/env python3
"""Per-request slow-path counts from run.py probe runs (crates/asmalloc/src/probe.rs on the
probe-counts branch):

  probesum.py SMALL_COUNT DIR...

For each run: the totals per request by kind, the (CPU, class) pairs with the most slow paths,
and, per class, the depot trips per request (a magazine with objects taken from the depot,
one sent to it full, an empty one taken or sent), fills and launders, with the CPUs that
take from the depot and those that send to it. A class whose magazines flow between CPUs
shows full magazines sent to the depot on some CPUs and taken on others: "moved" counts, per
class, what CPUs took from the depot beyond what they sent to it.
"""
import json
import sys
from pathlib import Path

KINDS = 16
NAMES = ["AllocSlow", "FreeSlow", "AllocSpare", "AllocDepot", "AllocFill", "EmptySpare", "EmptyDepot",
         "EmptyMade", "RetFullSpare", "RetFullDepot", "RetEmptySpare", "RetEmptyDepot", "StockAdmit", "Launder",
         "AllocInstLvl", "FreeDispLvl"]
K = {n: i for i, n in enumerate(NAMES)}



def size(ordinal):
    """The slot size of small class `ordinal` (crates/asmalloc/src/classes: 8-byte quantum,
    exact to 64 bytes, then four classes per doubling)."""
    index = ordinal + 1
    if index <= 8:
        return index * 8
    k = index + 3
    return ((5 + (k & 3)) << ((k >> 2) - 2)) * 8


small = int(sys.argv[1])
for d in sys.argv[2:]:
    for p in sorted(Path(d).glob("*.probe.json")):
        tag = p.name[: -len(".probe.json")]
        n = json.loads((p.parent / f"{tag}.result.json").read_text())["groups"]["m"]["lat_us"]["n"]
        tot = [0] * KINDS
        pairs, cls = {}, {}
        for i, v in ((int(k), v) for k, v in json.loads(p.read_text()).items()):
            kind, c, cpu = i % KINDS, (i // KINDS) % small, i // KINDS // small
            tot[kind] += v
            if kind in (K["AllocSlow"], K["FreeSlow"]):
                pairs[(cpu, c)] = pairs.get((cpu, c), 0) + v
            e = cls.setdefault(c, {"k": [0] * KINDS, "take": {}, "send": {}})
            e["k"][kind] += v
            if kind == K["AllocDepot"]:
                e["take"][cpu] = e["take"].get(cpu, 0) + v
            if kind == K["RetFullDepot"]:
                e["send"][cpu] = e["send"].get(cpu, 0) + v
        per = {NAMES[k]: round(tot[k] / n, 4) for k in range(14) if tot[k]}
        moved_all = 0
        for e in cls.values():
            moved_all += sum(max(0, v - e["send"].get(cpu, 0)) for cpu, v in e["take"].items())
        per["MovedBetweenCpus"] = round(moved_all / n, 4)
        print(f"\n{d.rstrip('/').split('/')[-1]}/{tag} ({n} requests): per request {per}")
        top = sorted(pairs.items(), key=lambda kv: -kv[1])[:6]
        print("  most slow paths, (cpu, class size): per request", [((k[0], size(k[1])), round(v / n, 3)) for k, v in top])
        print("  size   slow/req  full-from-depot/req  full-to-depot/req  moved/req  empty-depot/req  fill/req  launder/req"
              "  takers->senders (per 1k req)")
        for c, e in sorted(cls.items(), key=lambda kv: -(kv[1]["k"][0] + kv[1]["k"][1])):
            k = e["k"]
            slow = k[0] + k[1]
            if slow / n < 0.002:
                continue
            fl = " ".join(f"{cpu}:{v * 1000 / n:.1f}" for cpu, v in sorted(e["take"].items()))
            to = " ".join(f"{cpu}:{v * 1000 / n:.1f}" for cpu, v in sorted(e["send"].items()))
            # Full magazines that really changed CPU: what CPUs took beyond what they sent.
            moved = sum(max(0, v - e["send"].get(cpu, 0)) for cpu, v in e["take"].items())
            print(f"  {size(c):5d}  {slow / n:8.4f}  {k[K['AllocDepot']] / n:19.4f}  {k[K['RetFullDepot']] / n:17.4f}"
                  f"  {moved / n:9.4f}"
                  f"  {(k[K['EmptyDepot']] + k[K['RetEmptyDepot']]) / n:15.4f}  {k[K['AllocFill']] / n:8.4f}"
                  f"  {k[K['Launder']] / n:11.4f}  take[{fl}] send[{to}]")
