#!/usr/bin/env python3
"""A servers-thread session's runs, for docs/research/meili-peak.md: under load (`s70`), after
close, and the burst after the silence, one line a run.

  thread.py FILE...
"""
import json
import sys


def main():
    for f in sys.argv[1:]:
        for line in open(f):
            d = json.loads(line)
            if d.get("type") != "run":
                continue
            s = d["served"]
            g = s["groups"]
            segs = s["generator"]["segments"]
            w = [x for x in segs if x["name"] == "s70"]
            lo, hi = w[0]["begin_ms"], w[-1]["end_ms"]
            anon = [p[2] / 1024 for p in s["curve_kib"] if lo <= p[0] <= hi]
            idle = [x for x in segs if x["name"] == "idle"]
            end = [p[2] / 1024 for p in s["curve_kib"] if idle and p[0] >= idle[0]["end_ms"] - 2000]
            l, sv = g["s70"]["lat_us"], g["s70"]["server"]
            burst = []
            for name in ("b5", "b50", "b500"):
                if name in g:
                    bl, bs = g[name]["lat_us"], g[name]["server"]
                    burst.append(f"{name} max {bl['max'] / 1e3:.1f} p99 {bl['p99'] / 1e3:.1f} ms flt {bs['faults_per_req'] or 0:.0f}")
            print(f"{f.split('/')[-1]:12s} {d['allocator']:9s} valid={d['valid']!s:5s} "
                  f"Mcyc {sv['cycles_per_req'] / 1e6:.2f} flt {sv['faults_per_req']:.1f} "
                  f"p99/p99.9 {l['p99'] / 1e3:.1f}/{l['p999'] / 1e3:.1f} "
                  f"anon {sum(anon) / len(anon):.0f}/{max(anon):.0f} close+60 {end[-1] if end else 0:.0f} | "
                  + " | ".join(burst))


if __name__ == "__main__":
    main()
