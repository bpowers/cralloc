#!/usr/bin/env python3
"""Programs of the screening's tree (tpchgen, jaq, rust-analyzer) under a preload library, one
arm a setting of ASMALLOC_WBH or another allocator's library, interleaved, for
docs/research/whole-block-huge.md. As grown-slots/tpch.py: a 300 ms spin on the program's
CPUs, then perf stat around walltime, under scripts/memlimit; and, beside it, the program's
AnonHugePages sampled, the machine's free 2 MiB blocks before the run and its thp_ counters
over it.

  pre.py OUT.jsonl --prog tpchgen --rounds N --arms 'main=LIB:m0f002d0,mi=MILIB:,glibc=-:'

LIB is a preload library ("-" for glibc). RUST_SCREEN_ROOT names the screening's tree,
WBH_WORK a work directory on disk. A measurement: run under scripts/measure --both-l3.
"""
import argparse
import json
import os
import random
import subprocess
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import wbhlib  # noqa: E402

ROOT = Path(__file__).resolve().parents[3]
MT, ST = "0-3,8-11", "2"
RAW = ",".join(f"r{c.split('=')[1][2:]}:u" for c in wbhlib.EVENTS.split(","))
EVENTS = "cycles,cycles:u,task-clock,page-faults," + RAW

PROGS = {
    "tpchgen": {"setup": "rm -rf out", "cmd": ["{bin}/tpchgen/tpchgen-cli", "parquet", "-s", "1", "--output-dir=out"],
                "cpus": MT},
    "jaq": {"cmd": ["{bin}/jaq/jaq", "-c",
                    ".features | map(select(.properties.BLOCK_NUM != null)) | group_by(.properties.STREET)"
                    " | map({{s: .[0].properties.STREET, n: length, lots: map(.properties.LOT_NUM)}}) | length",
                    "{inp}/dl/citylots.json"], "cpus": ST},
    "rust-analyzer": {"cmd": ["{bin}/rust-analyzer/rust-analyzer", "analysis-stats", "--disable-build-scripts",
                              "--disable-proc-macros", "{inp}/ra/ripgrep-15.2.0"],
                      "env": {"RUST_SRC_PATH": "{inp}/rust-src-1.99.1/rust-src/lib/rustlib/src/rust/library",
                              "CARGO_NET_OFFLINE": "true", "CARGO": "{root}/toolchain-cargo"},
                      "cpus": MT},
}


def parse_stat(path):
    vals = {}
    for line in Path(path).read_text().splitlines():
        f = line.split(",")
        if len(f) < 3 or line.startswith("#"):
            continue
        try:
            vals[f[2]] = float(f[0])
        except ValueError:
            vals[f[2]] = None
    return vals


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("out")
    ap.add_argument("--prog", required=True, choices=sorted(PROGS))
    ap.add_argument("--rounds", type=int, default=3)
    ap.add_argument("--arms", required=True)
    ap.add_argument("--diag", action="store_true")
    a = ap.parse_args()
    arms = {}
    for spec in a.arms.split(","):
        name, rest = spec.split("=", 1)
        lib, _, setting = rest.rpartition(":")
        arms[name] = (lib, setting or "m0f002d0")
    R = Path(os.environ["RUST_SCREEN_ROOT"])
    W = Path(os.environ["WBH_WORK"]) / a.prog
    W.mkdir(parents=True, exist_ok=True)
    P = PROGS[a.prog]
    fill = lambda s: s.format(bin=R / "bin", inp=R / "inputs", root=R, work=W)  # noqa: E731
    for r in range(a.rounds):
        order = list(arms)
        random.shuffle(order)
        for name in order:
            lib, setting = arms[name]
            stat, wall, dg, pidf = W / "stat", W / "wall", W / "diag", W / "pid"
            for p in (stat, wall, dg, pidf):
                p.unlink(missing_ok=True)
            if "setup" in P:
                subprocess.run(P["setup"], shell=True, cwd=W, check=True)
            env = dict(os.environ, ASMALLOC_WBH=setting)
            env.update({k: fill(v) for k, v in P.get("env", {}).items()})
            if a.diag:
                env["WBH_DIAG"] = str(dg)
            pre = ["env", f"LD_PRELOAD={lib}"] if lib != "-" else ["env"]
            if "tcmalloc" in lib:
                # Per-CPU mode needs glibc's rseq registration off (bench/allocators.toml).
                pre.append("GLIBC_TUNABLES=glibc.pthread.rseq=0")
            subprocess.run([str(R / "tools/spin"), P["cpus"], "300"], check=True)
            argv = [str(ROOT / "scripts/memlimit"), "-m", "8G", "-t", "10m", "--",
                    "perf", "stat", "-x,", "-o", str(stat), "-e", EVENTS, "--",
                    "taskset", "-c", P["cpus"], str(R / "tools/walltime"), str(wall)] + \
                wbhlib.pid_wrapper(pidf) + pre + [fill(c) for c in P["cmd"]]
            free = wbhlib.free_huge()
            v0 = wbhlib.vmstat_thp()
            sampler = wbhlib.Sampler(pidf, ms=200)
            t0 = time.time()
            p = subprocess.run(argv, env=env, cwd=W, timeout=900, capture_output=True, text=True)
            smaps = sampler.finish()
            thp = wbhlib.delta(v0, wbhlib.vmstat_thp())
            rec = {"prog": a.prog, "arm": name, "setting": setting, "round": r, "t": t0, "rc": p.returncode,
                   "wall": float(wall.read_text()) if wall.exists() else None, "stat": parse_stat(stat),
                   "smaps": smaps, "free_2m": free, "thp": thp,
                   "memlimit": p.stderr.strip().splitlines()[-1:]}
            if dg.exists():
                rec["diag"] = dg.read_text()
            with open(a.out, "a") as f:
                f.write(json.dumps(rec) + "\n")
            st = rec["stat"]
            g = lambda k: st.get(k) or 0  # noqa: E731
            print(f"{a.prog} {name:8} r{r} rc={p.returncode} wall {rec['wall'] or 0:.3f} cyc {g('cycles')/1e9:.2f}G "
                  f"user {g('cycles:u')/1e9:.2f}G pf {int(g('page-faults'))} miss {g(RAW.split(',')[0])/1e6:.1f}M "
                  f"w4k {g(RAW.split(',')[1])/1e6:.2f}M w2m {g(RAW.split(',')[2])/1e6:.2f}M "
                  f"anonpk {smaps.get('anon_peak', 0)/1024:.0f} hugepk {smaps.get('huge_peak', 0)/1024:.0f} "
                  f"free2m {free} thpfa {thp.get('thp_fault_alloc', 0)}", flush=True)


if __name__ == "__main__":
    main()
