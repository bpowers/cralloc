#!/usr/bin/env python3
"""What the climb out of each trough costs in drift runs: for every high phase after the
first, its first sample's time a million operations and minor faults, against the same for
the phase's last sample (the steady part), and the run's switch counters.

    python3 -I bench/probes/huge-tail/rampcost.py FILE...
"""
import json
import re
import sys
from pathlib import Path


def main():
    print("| run | climbs | first sample ms/Mop | steady ms/Mop | first faults | steady faults | troughs heap MiB | advice down+up, splits, MiB returned |")
    print("|---|---|---|---|---|---|---|---|")
    for f in sys.argv[1:]:
        s = [json.loads(l[6:]) for l in Path(f).read_text().splitlines() if l.startswith("DRIFT ")]
        s = [x for x in s if x["kind"] == "load"]
        firsts, lasts, lows = [], [], []
        for i in range(1, len(s)):
            d = lambda j: ((s[j]["t_ms"] - s[j - 1]["t_ms"]) / ((s[j]["ops"] - s[j - 1]["ops"]) / 1e6), s[j]["minflt"] - s[j - 1]["minflt"])
            starts = s[i]["phase"] != s[i - 1]["phase"]
            if s[i]["phase"].startswith("hi") and starts and i > 2:
                firsts.append(d(i))
                if i + 1 < len(s) and s[i + 1]["phase"] == s[i]["phase"]:
                    lasts.append(d(i + 1))
            if s[i]["phase"].startswith("lo") and (i + 1 == len(s) or s[i + 1]["phase"] != s[i]["phase"]):
                lows.append(s[i]["heap"] / 2**20)
        m = lambda xs, k: sum(x[k] for x in xs) / len(xs) if xs else float("nan")
        ht = ""
        p = Path(f + ".ht")
        if p.exists() and p.read_text().strip():
            kv = dict(re.findall(r"(\w+)=(\d+)", p.read_text().splitlines()[-1]))
            ht = f"{kv['unadvise']}+{kv['readvise']}, {kv['splits']}, {int(kv['tail_pages']) * 4 / 1024:.0f}"
        print(f"| {Path(f).parent.name}/{Path(f).name} | {len(firsts)} | {m(firsts, 0):.1f} | {m(lasts, 0):.1f} | {m(firsts, 1):.0f} | {m(lasts, 1):.0f} | {sum(lows) / max(1, len(lows)):.0f} | {ht} |")


if __name__ == "__main__":
    main()
