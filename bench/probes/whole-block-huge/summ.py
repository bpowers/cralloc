#!/usr/bin/env python3
"""Medians by arm of the drivers' records, for docs/research/whole-block-huge.md.

  summ.py kern RUNS.jsonl...      kern.py: Mcycles, faults, dTLB misses and walks (M), peak
                                   AnonHugePages and cgroup peak (MiB), free 2 MiB blocks
  summ.py pre RUNS.jsonl...       pre.py: the same from perf stat, cycles user and kernel
  summ.py sparse RUNS.jsonl...    sparse.py: kcycles a block, cycles a read, anonymous and
                                   huge MiB held, faults a life; lives after the first
Each arm's cycles are also given against the arm named `main`, and the range of the runs.
"""
import json
import statistics as st
import sys
from collections import defaultdict


def mib(peak):
    if not peak:
        return 0
    v = peak.split()[0]
    return float(v[:-1]) * {"K": 1 / 1024, "M": 1, "G": 1024}.get(v[-1], 1)


def table(rows, cols, key0="main"):
    groups = defaultdict(dict)
    for (job, arm), v in rows.items():
        groups[job][arm] = v
    for job, arms in groups.items():
        print(job)
        base = st.median(arms[key0]["cyc"]) if key0 in arms else None
        for arm, v in arms.items():
            c = st.median(v["cyc"])
            rel = f"{100 * (c / base - 1):+5.1f}%" if base else ""
            extra = " ".join(f"{k} {st.median(v[k]):.{d}f}" for k, d in cols if k in v)
            print(f"  {arm:6} n={len(v['cyc'])} cyc {c:9.2f} ({min(v['cyc']):.2f}-{max(v['cyc']):.2f}) {rel:7} {extra}")


def kern(paths):
    rows = defaultdict(lambda: defaultdict(list))
    for p in paths:
        for line in open(p):
            r = json.loads(line)
            if not r.get("result"):
                continue
            ph = r["result"]["phases"].values()
            t = lambda k: sum(x.get(k, 0) for x in ph)  # noqa: E731
            v = rows[(r["job"], r["arm"])]
            v["cyc"].append(t("cycles") / 1e6)
            v["faults_k"].append(t("page_faults") / 1e3)
            v["miss_M"].append(t("dtlb_miss") / 1e6)
            v["walk4k_M"].append(t("walk_4k") / 1e6)
            v["walk2m_M"].append(t("walk_2m") / 1e6)
            v["huge_MiB"].append((r.get("smaps") or {}).get("huge_peak", 0) / 1024)
            v["peak_MiB"].append(mib(r.get("memlimit")))
            v["free2m"].append(r.get("free_2m", 0))
            v["thpfa"].append(r.get("thp", {}).get("thp_fault_alloc", 0))
    table(rows, [("faults_k", 1), ("miss_M", 2), ("walk4k_M", 2), ("walk2m_M", 3), ("huge_MiB", 0), ("peak_MiB", 0),
                 ("free2m", 0), ("thpfa", 0)])


def pre(paths):
    rows = defaultdict(lambda: defaultdict(list))
    for p in paths:
        for line in open(p):
            r = json.loads(line)
            s = r["stat"]
            g = lambda k: s.get(k) or 0  # noqa: E731
            v = rows[(r["prog"], r["arm"])]
            v["cyc"].append(g("cycles") / 1e9)
            v["user_G"].append(g("cycles:u") / 1e9)
            v["kern_G"].append((g("cycles") - g("cycles:u")) / 1e9)
            v["faults_k"].append(g("page-faults") / 1e3)
            v["miss_M"].append(g("rff45:u") / 1e6)
            v["walk4k_M"].append(g("r1045:u") / 1e6)
            v["walk2m_M"].append(g("r4045:u") / 1e6)
            v["anon_MiB"].append((r.get("smaps") or {}).get("anon_peak", 0) / 1024)
            v["huge_MiB"].append((r.get("smaps") or {}).get("huge_peak", 0) / 1024)
            v["free2m"].append(r.get("free_2m", 0))
    table(rows, [("user_G", 2), ("kern_G", 2), ("faults_k", 0), ("miss_M", 1), ("walk4k_M", 2), ("walk2m_M", 2),
                 ("anon_MiB", 0), ("huge_MiB", 0), ("free2m", 0)])


def sparse(paths):
    rows = defaultdict(lambda: defaultdict(list))
    for p in paths:
        for line in open(p):
            r = json.loads(line)
            v = rows[(r["case"], r["arm"])]
            for ln in r["lines"]:
                f = ln.split()
                if f[0] != "life" or f[1] == "0":
                    continue
                d = dict(zip(f[2::2], f[3::2]))
                v["cyc"].append(float(d["kcyc_per_block"]))
                v["read_cyc"].append(float(d["cyc_per_read"]))
                v["anon_MiB"].append(float(d["anon"]))
                v["huge_MiB"].append(float(d["huge"]))
                v["faults"].append(float(d["faults"]))
                v["free2m"].append(r["free_2m"])
                v["stall"].append(r.get("thp", {}).get("compact_stall", 0))
    table(rows, [("read_cyc", 0), ("anon_MiB", 1), ("huge_MiB", 1), ("faults", 0), ("free2m", 0), ("stall", 0)])


if __name__ == "__main__":
    {"kern": kern, "pre": pre, "sparse": sparse}[sys.argv[1]](sys.argv[2:])
