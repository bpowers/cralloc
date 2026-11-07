#!/usr/bin/env python3
"""Tables from sessions of the servers-thread suite (session.sh): threadsum.py DIR [--runs]

Reads benchrun's results, every *.jsonl and *.jsonl.gz in DIR. Per job and allocator, medians
over the valid runs:

- work and tails at 70%: server cycles a request, p50, p99, p99.9, missed slots, page faults.
- memory: resident and anonymous peak under load (benchrun's 250 ms curve), the anonymous mean
  at 70%, the cgroup peak; resident and anonymous at the end of the 70% load, 10 and 29.5 s
  into the silence and 10, 30 and 59.5 s after the connections closed.
- the burst after the silence: latency of the requests due in its first 5 ms, 5 to 50 ms and
  50 to 500 ms, and the server's cycles a request over its first 500 ms.

--runs also prints each run's figures.
"""
import gzip
import json
import statistics
import sys
from collections import defaultdict
from pathlib import Path

MiB = 2 ** 20


def load(d):
    runs = []
    files = sorted(Path(d).glob("*.jsonl")) + sorted(Path(d).glob("*.jsonl.gz"))
    for f in files:
        lines = gzip.open(f, "rt") if f.suffix == ".gz" else open(f)
        for l in lines:
            r = json.loads(l)
            if r.get("type") == "run":
                runs.append(r)
    return runs


def curve_at(r, t_rel_ms, col):
    v = [p[col] for p in r["served"]["curve_kib"] if p[0] <= t_rel_ms]
    return v[-1] * 1024 if v else None


def one(r):
    segs = r["served"]["generator"]["segments"]
    seg = lambda n: [s for s in segs if s["name"] == n]
    g = r["served"]["groups"]
    s70 = seg("s70")
    a70, b70 = s70[0]["begin_ms"] / 1e3, s70[-1]["end_ms"] / 1e3
    off, idle = seg("off")[0], seg("idle")[0]
    o0, i0 = off["begin_ms"] / 1e3, idle["begin_ms"] / 1e3
    burst0 = seg("b5")[0]["begin_ms"] / 1e3
    res = {
        "cycles": g["s70"]["server"]["cycles_per_req"],
        "p50": g["s70"]["lat_us"]["p50"], "p99": g["s70"]["lat_us"]["p99"], "p999": g["s70"]["lat_us"]["p999"],
        "missed": g["s70"]["missed"],
        "faults": g["s70"]["server"]["faults_per_req"],
    }
    for b in ("b5", "b50", "b500", "b2"):
        res[f"{b}_p50"] = g[b]["lat_us"]["p50"]
        res[f"{b}_p99"] = g[b]["lat_us"]["p99"]
        res[f"{b}_max"] = g[b]["lat_us"]["max"]
        res[f"{b}_n"] = g[b]["lat_us"]["n"]
    # Cycles a request over the burst's first 500 ms (b5, b50 and b500 together).
    cyc = sum(g[b]["server"]["cycles_per_req"] * g[b]["lat_us"]["n"] for b in ("b5", "b50", "b500") if g[b]["lat_us"]["n"])
    n = sum(g[b]["lat_us"]["n"] for b in ("b5", "b50", "b500"))
    res["burst_cycles"] = cyc / n if n else None
    curve = r["served"]["curve_kib"]
    loaded = [p for p in curve if p[0] < o0 * 1e3 or burst0 * 1e3 <= p[0] < i0 * 1e3]
    res["peak_rss"] = max(p[1] for p in loaded) * 1024
    res["peak_anon"] = max(p[2] for p in loaded) * 1024
    steady = [p[2] for p in curve if a70 * 1e3 <= p[0] < b70 * 1e3]
    res["mean_anon_s70"] = sum(steady) / len(steady) * 1024
    for name, t in (("off10", o0 + 10), ("off29", o0 + 29.5), ("idle10", i0 + 10), ("idle30", i0 + 30),
                    ("idle59", i0 + 59.5), ("end70", b70)):
        res[f"rss_{name}"] = curve_at(r, t * 1e3, 1)
        res[f"anon_{name}"] = curve_at(r, t * 1e3, 2)
    res["cgroup_peak"] = r["cgroup"]["peak"]
    return res


def med(xs):
    xs = [x for x in xs if x is not None]
    return statistics.median(xs) if xs else None


def fmt(v, scale=1.0, d=0):
    return "-" if v is None else f"{v / scale:.{d}f}"


def main():
    runs = load(sys.argv[1])
    by = defaultdict(lambda: defaultdict(list))
    for r in runs:
        if not r.get("valid"):
            # The burst's first two pieces last 5 and 45 ms, over which half a millisecond of
            # anything else on the server's CPUs crosses the runner's limit; such a run is
            # kept, and listed, when nothing else was wrong with it.
            short = all("other work on the server's CPUs" in p and ("segment b5 " in p or "segment b50 " in p)
                        for p in r.get("problems", []))
            print(f"{'kept' if short else 'invalid'}: {r['job']} {r['allocator']} {r.get('problems')}")
            if not short:
                continue
        res = one(r)
        by[r["job"]][r["allocator"]].append(res)
        if "--runs" in sys.argv:
            print(r["job"], r["allocator"], json.dumps({k: (round(v, 3) if isinstance(v, float) else v) for k, v in res.items()}))
    for job, allocs in by.items():
        print(f"\n## {job}")
        rows = [
            ("runs", lambda rs: str(len(rs))),
            ("kcycles/req at 70%", lambda rs: fmt(med([x["cycles"] for x in rs]), 1e3, 1)),
            ("p50 / p99 / p99.9 us", lambda rs: " / ".join(fmt(med([x[k] for x in rs])) for k in ("p50", "p99", "p999"))),
            ("missed at 70%", lambda rs: fmt(med([x["missed"] for x in rs]))),
            ("page faults/req at 70%", lambda rs: fmt(med([x["faults"] for x in rs]), 1, 2)),
            ("mean anon at 70%, MiB", lambda rs: fmt(med([x["mean_anon_s70"] for x in rs]), MiB)),
            ("peak resident under load, MiB", lambda rs: fmt(med([x["peak_rss"] for x in rs]), MiB)),
            ("peak anon under load, MiB", lambda rs: fmt(med([x["peak_anon"] for x in rs]), MiB)),
            ("cgroup peak, MiB", lambda rs: fmt(med([x["cgroup_peak"] for x in rs]), MiB)),
        ]
        for name in ("end70", "off10", "off29", "idle10", "idle30", "idle59"):
            rows.append((f"rss / anon at {name}, MiB", lambda rs, n=name: " / ".join(
                fmt(med([x[f"{k}_{n}"] for x in rs]), MiB) for k in ("rss", "anon"))))
        for b in ("b5", "b50", "b500", "b2"):
            rows.append((f"{b} n, p50 / p99 / max us", lambda rs, b=b: fmt(med([x[f"{b}_n"] for x in rs])) + ": " + " / ".join(
                fmt(med([x[f"{b}_{k}"] for x in rs])) for k in ("p50", "p99", "max"))))
        rows.append(("burst first 500 ms kcycles/req", lambda rs: fmt(med([x["burst_cycles"] for x in rs]), 1e3, 1)))
        names = sorted(allocs)
        w = max(len(r[0]) for r in rows)
        print(f"{'':{w}}  " + "  ".join(f"{a:>26}" for a in names))
        for label, f in rows:
            print(f"{label:{w}}  " + "  ".join(f"{f(allocs[a]):>26}" for a in names))


if __name__ == "__main__":
    main()
