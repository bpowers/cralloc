#!/usr/bin/env python3
"""Summarises the server ladder (servers.sh) for docs/research/large-heap-constants.md.

    python3 -I srvsum.py DIR [--diag]

For each rung and allocator, over the rounds: server cycles a request, p50, p99, p99.9 and the
slowest request at 70% (`s70`), requests that missed their slot, page faults a thousand
requests; resident over live at the end of the minute at 70% and 9.75 s after the connections
close (live from the live-bytes run of the same round, at the same mark); and over the bursts
after silences (`on`), the first 2,000 requests' p99.9 (`head`) and page faults a request. With
--diag, from the watcher, the diagnostic build's step counters (summary.py's kinds) and every
server's page tables and mappings at their most.
"""
import json
import statistics
import sys
from collections import defaultdict
from pathlib import Path

sys.dont_write_bytecode = True
sys.path.insert(0, str(Path(__file__).resolve().parent))
from summary import KINDS, SCALARS, kind_summary, ns  # noqa: E402


def mark(served, name, nth=-1):
    ms = [m for m in served["marks_ms"] if m[1] == name]
    return ms[nth] if ms else None


def main():
    d = Path(sys.argv[1])
    runs = defaultdict(list)
    live = {}
    for f in sorted(d.glob("orders-*.jsonl")):
        if f.name.endswith(".watch.jsonl"):
            continue
        rung = f.name.split("-")[1]
        rnd = f.stem.split("-r")[-1]
        for line in f.read_text().splitlines():
            r = json.loads(line)
            if "served" not in r:
                continue
            if r["allocator"] == "live-bytes":
                live[(rung, rnd)] = r["served"]["marks_ms"]
            runs[(rung, r["allocator"])].append((rnd, r))
    order = {"0.5G": 0, "2G": 1, "4G": 2, "10G": 3}
    for (rung, alloc), rs in sorted(runs.items(), key=lambda kv: (order.get(kv[0][0], 9), kv[0][1])):
        if alloc == "live-bytes":
            continue
        rows = []
        for rnd, r in rs:
            s = r["served"]
            g = s["groups"]["s70"]
            on = s["groups"].get("on", {})
            lm = live.get((rung, rnd))
            # The end of the minute at 70%: the last s70 piece's end, which is the first `off`
            # mark; and the end of the idle close.
            end70 = mark(s, "off", 0)
            endidle = mark(s, "end")
            ratio70 = ratio_idle = None
            if lm and end70:
                lv = [m for m in lm if m[1] == "off"][0][2]
                ratio70 = rss_at_curve(s, end70[0] - 100) / lv
            if lm and endidle:
                lv = [m for m in lm if m[1] == "end"][0][2]
                ratio_idle = rss_at_curve(s, endidle[0] - 250) / lv
            rows.append({
                "valid": r.get("valid", True),
                "cycles_req": g["server"]["cycles_per_req"],
                "p50": g["lat_us"]["p50"], "p99": g["lat_us"]["p99"], "p999": g["lat_us"]["p999"],
                "max": g["lat_us"]["max"], "missed": g["missed"],
                "faults_kreq": 1000 * (g["server"]["faults_per_req"] or 0),
                "ratio70": ratio70, "ratio_idle": ratio_idle,
                "burst_head_p999": on.get("head", {}).get("p999"), "burst_max": on.get("lat_us", {}).get("max"),
                "burst_faults_req": on.get("server", {}).get("faults_per_req"),
                "peak_mib": r["cgroup"]["peak"] / 2**20,
                "rate": g["rate"],
            })
        def med(k):
            xs = [x[k] for x in rows if x[k] is not None]
            return round(statistics.median(xs), 4) if xs else None
        out = {k: med(k) for k in rows[0]}
        out["rounds"] = len(rows)
        out["invalid"] = sum(not x["valid"] for x in rows)
        print(rung, alloc, json.dumps(out))
    if "--diag" in sys.argv:
        for w in sorted(d.glob("orders-*.watch.jsonl")):
            last = {}
            peak = {}
            for line in w.read_text().splitlines():
                r = json.loads(line)
                key = (r["exe"].split("/target/")[1].split("/")[0], r["pid"])
                last[key] = r
                p = peak.setdefault(key, {"pte": 0, "maps": 0, "rss": 0})
                p["pte"] = max(p["pte"], r.get("pte") or 0)
                p["maps"] = max(p["maps"], r.get("maps") or 0)
                p["rss"] = max(p["rss"], r.get("rss_anon") or 0)
            for key, r in last.items():
                pk = peak[key]
                print(w.name, key[0], "pte_mib", round(pk["pte"] / 2**20, 1), "maps", pk["maps"],
                      "rss_gib", round(pk["rss"] / 2**30, 2), "bg_ms", (r.get("bg_ns") or 0) / 1e6, "bg_wake", r.get("bg_vol"))
                if "diag" in r:
                    diag = {int(k): v for k, v in r["diag"].items()}
                    for k, name in enumerate(KINDS):
                        ks = kind_summary(diag, k)
                        if ks:
                            print("   ", name, json.dumps(ks))
                    sc = {name: diag.get(i, 0) for name, i in SCALARS.items()}
                    sc["last_lap_ms"] = round(ns(sc.pop("last_lap_ticks")) / 1e6, 1)
                    print("   ", json.dumps(sc))


def rss_at_curve(served, t_ms):
    """Resident bytes from the 250 ms curve, its last sample at or before `t_ms` (the curve and
    the marks share one clock, as bench/probes/partial-load/largesum.py reads them)."""
    v = [p[1] for p in served["curve_kib"] if p[0] <= t_ms]
    return v[-1] * 1024 if v else None


if __name__ == "__main__":
    main()
