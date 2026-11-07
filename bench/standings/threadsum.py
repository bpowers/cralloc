#!/usr/bin/env python3
"""Tables for docs/research/default-thread-servers.md from sessions of the servers-thread suite
(session.sh): benchrun's results (TAG.jsonl) and the watcher's samples (TAG.watch.jsonl).

  summarize.py DIR [--runs]

Per job and allocator, medians over the valid runs (and ranges where they matter):

- thread: whether the background thread existed, when it was first seen (seconds from the
  start of the plan; negative is before the load began), its wake-ups a second over the
  minute's pieces at 70% (`s70`, from its voluntary context switches), its CPU time there,
  and how long after the silence (`off`) and after the connections closed (`idle`) it made
  its last wake-up, with the wake-ups in the last 10 s of each.
- work and tails at 70%: server cycles a request, p50, p99, p99.9.
- memory: resident and anonymous peak under load (benchrun's 250 ms curve), resident,
  anonymous and the cgroup's anonymous memory 10 and 29.5 s into the silence and 10, 30 and
  59.5 s after the connections closed.
- the burst after the silence: latency of the requests due in its first 5 ms, 5 to 50 ms and
  50 to 500 ms, and the server's cycles a request over its first 500 ms.
"""
import gzip
import json
import os
import statistics
import sys
from collections import defaultdict
from pathlib import Path

MiB = 2 ** 20


def load(d):
    """Every *.jsonl and *.jsonl.gz in `d`: the watcher's when the name has `watch` in it."""
    runs, watch = [], []
    files = sorted(Path(d).glob("*.jsonl")) + sorted(Path(d).glob("*.jsonl.gz"))
    for f in files:
        lines = gzip.open(f, "rt") if f.suffix == ".gz" else open(f)
        if "watch" in f.name:
            watch.extend(json.loads(l) for l in lines if l.strip())
        else:
            for l in lines:
                r = json.loads(l)
                if r.get("type") == "run":
                    runs.append(r)
    return runs, watch


def plan_start(pid, watch):
    """The plan's start in wall-clock seconds: the start of the first load generator after the
    server's (the generator reads its requests and connects first, so this is early by that
    much, well under a sample's 250 ms for these jobs)."""
    start = next(w["start"] for w in watch if w["type"] == "proc" and w["pid"] == pid)
    gens = [w["start"] for w in watch if w["type"] == "gen" and w["start"] >= start]
    return min(gens) if gens else None


def match(r, watch):
    exe = os.path.realpath(r["argv"][0])
    want_off = r["allocator"] == "asmalloc-off"
    best = None
    for w in watch:
        if w["type"] != "proc" or w["exe"] != exe:
            continue
        if (w.get("background_env") == "0") != want_off:
            continue
        dt = abs(w["start"] - r["started"])
        if dt < 3 and (best is None or dt < best[0]):
            best = (dt, w["pid"])
    if best is None:
        return None, None
    pid = best[1]
    return [w for w in watch if w["type"] == "s" and w["pid"] == pid], plan_start(pid, watch)


def at(samples, t, key):
    v = [s for s in samples if s["t"] <= t]
    return v[-1][key] if v else None


def curve_at(r, t_rel_ms, col):
    v = [p[col] for p in r["served"]["curve_kib"] if p[0] <= t_rel_ms]
    return v[-1] * 1024 if v else None


def thread_series(samples):
    """(t, voluntary, cpu ticks) of the background thread at each sample where it exists."""
    out = []
    for s in samples:
        if s["threads"]:
            vol = sum(v[0] for v in s["threads"].values())
            cpu = sum(v[2] for v in s["threads"].values())
            out.append((s["t"], vol, cpu))
    return out


def last_change(series, a, b):
    """Seconds after `a` of the last sample in [a, b) whose count differs from the one
    before it, and the number of wake-ups in the last 10 s of the window."""
    pts = [p for p in series if a - 0.5 <= p[0] < b]
    last = None
    for p, q in zip(pts, pts[1:]):
        if q[1] != p[1] and q[0] >= a:
            last = q[0] - a
    tail = [p for p in pts if p[0] >= b - 10]
    tail_n = tail[-1][1] - tail[0][1] if len(tail) > 1 else None
    return last, tail_n


def one(r, matched):
    samples, p0 = matched
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
        res[f"cg_{name}"] = at(samples, p0 + t, "cg_anon") if samples and p0 else None
    res["cgroup_peak"] = r["cgroup"]["peak"]
    if samples and p0:
        ts = thread_series(samples)
        res["thread"] = bool(ts)
        if ts:
            res["first_seen"] = ts[0][0] - p0
            w = [p for p in ts if p0 + a70 <= p[0] <= p0 + b70]
            if len(w) > 1:
                dt = w[-1][0] - w[0][0]
                res["wakes_s70"] = (w[-1][1] - w[0][1]) / dt
                res["cpu_ms_s70"] = (w[-1][2] - w[0][2]) * 1e3 / os.sysconf("SC_CLK_TCK") / dt
            res["park_off"], res["tail_off"] = last_change(ts, p0 + o0, p0 + burst0)
            res["park_idle"], res["tail_idle"] = last_change(ts, p0 + i0, p0 + i0 + 60)
            res["wakes_total"] = ts[-1][1]
            res["cpu_s_total"] = ts[-1][2] / os.sysconf("SC_CLK_TCK")
    return res


def med(xs):
    xs = [x for x in xs if x is not None]
    return statistics.median(xs) if xs else None


def fmt(v, scale=1.0, d=0):
    return "-" if v is None else f"{v / scale:.{d}f}"


def main():
    runs, watch = load(sys.argv[1])
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
        res = one(r, match(r, watch))
        by[r["job"]][r["allocator"]].append(res)
        if "--runs" in sys.argv:
            print(r["job"], r["allocator"], json.dumps({k: (round(v, 3) if isinstance(v, float) else v) for k, v in res.items()}))
    for job, allocs in by.items():
        print(f"\n## {job}")
        rows = [
            ("runs", lambda rs: str(len(rs))),
            ("thread seen", lambda rs: ",".join("y" if x.get("thread") else "n" for x in rs)),
            ("first seen, s from plan start", lambda rs: fmt(med([x.get("first_seen") for x in rs]), 1, 1)),
            ("wake-ups/s at 70%", lambda rs: fmt(med([x.get("wakes_s70") for x in rs]), 1, 2)),
            ("thread CPU ms/s at 70%", lambda rs: fmt(med([x.get("cpu_ms_s70") for x in rs]), 1, 1)),
            ("last wake-up after silence starts, s", lambda rs: ",".join(fmt(x.get("park_off"), 1, 1) for x in rs)),
            ("wake-ups in last 10 s of silence", lambda rs: ",".join(fmt(x.get("tail_off")) for x in rs)),
            ("last wake-up after close, s", lambda rs: ",".join(fmt(x.get("park_idle"), 1, 1) for x in rs)),
            ("wake-ups in last 10 s after close", lambda rs: ",".join(fmt(x.get("tail_idle")) for x in rs)),
            ("wake-ups over the run", lambda rs: fmt(med([x.get("wakes_total") for x in rs]))),
            ("thread CPU s over the run", lambda rs: fmt(med([x.get("cpu_s_total") for x in rs]), 1, 2)),
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
            rows.append((f"rss / anon / cg anon at {name}, MiB", lambda rs, n=name: " / ".join(
                fmt(med([x[f"{k}_{n}"] for x in rs]), MiB) for k in ("rss", "anon", "cg"))))
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
