import json, sys, collections, statistics
rows = collections.defaultdict(list)
for f in sys.argv[1:]:
    for l in open(f):
        try:
            r = json.loads(l)
        except Exception:
            continue
        if "job" not in r or not r.get("served"):
            continue
        job = r["job"] if isinstance(r["job"], str) else r["job"].get("id")
        a = r["allocator"] if isinstance(r["allocator"], str) else r["allocator"].get("name")
        g = r["served"]["groups"].get("s70", {})
        s = g.get("server", {})
        if not s:
            continue
        rows[(job, a)].append(dict(s, p99=g.get("lat_us", {}).get("p99"), valid=r.get("valid")))
keys = ["cycles_per_req", "instructions_per_req", "dmnd_all_per_req", "l2_per_req", "ccx_per_req", "p99"]
for (job, a), rs in sorted(rows.items()):
    med = {k: statistics.median([x[k] for x in rs if x.get(k) is not None] or [float("nan")]) for k in keys}
    print(f"{job:16} {a:10} n={len(rs)} valid={[x['valid'] for x in rs]} " + " ".join(f"{k.replace('_per_req','')}={med[k]:.1f}" for k in keys)
          + f"  L1miss-L2hit={med['dmnd_all_per_req']-med['l2_per_req']:.1f}")
