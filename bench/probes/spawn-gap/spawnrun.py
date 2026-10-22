#!/usr/bin/env python3
"""Run the spawn workload for several builds, interleaved, and print per-task counters.

usage: spawnrun.py PLACE ROUNDS BUILD[,BUILD...] [EVENTS] [extra workload args...]
  PLACE   t4 (CPUs 0-3) or t8 (0-3,8-11)
  BUILD   name=path-to-workload
  EVENTS  BENCH_EVENTS value, or - for none
"""
import json, os, random, subprocess, sys

WT = str(__import__("pathlib").Path(__file__).resolve().parents[3])
place, rounds, builds = sys.argv[1], int(sys.argv[2]), sys.argv[3]
events = sys.argv[4] if len(sys.argv) > 4 else "-"
extra = sys.argv[5:]
def spec(b):
    # label[:dir[:ENV=VAL;ENV=VAL]]
    parts = b.split(":", 2)
    label = parts[0]
    d = parts[1] if len(parts) > 1 and parts[1] else label
    envs = dict(kv.split("=", 1) for kv in parts[2].split(";")) if len(parts) > 2 and parts[2] else {}
    return label, f"{WT}/bench/target/run/{d}/workload", envs
builds = [spec(b) for b in builds.split("/")]
cpus, threads = {"t4": ("0-3", 4), "t8": ("0-3,8-11", 8), "t2": ("0-1", 2), "t1": ("2", 1)}[place]
args = ["spawn", f"threads={threads}", "rounds=20000", "sizes=small"] + extra
env = dict(os.environ, BENCH_CPUS=cpus, MEMLIMIT_QUIET="1")
if events != "-":
    env["BENCH_EVENTS"] = events
else:
    env.pop("BENCH_EVENTS", None)
out = os.environ.get("SPAWN_OUT")
rows = {}
for r in range(rounds):
    order = builds[:]
    random.shuffle(order)
    for name, path, benv in order:
        cmd = [f"{WT}/scripts/memlimit", "-m", "2G", "-t", "120", "--", "taskset", "-c", cpus, path] + args
        p = subprocess.run(cmd, env=dict(env, **benv), capture_output=True, text=True)
        line = [l for l in p.stdout.splitlines() if l.startswith("BENCH_RESULT ")]
        if not line:
            print(name, "FAILED", p.returncode, p.stderr[-2000:], file=sys.stderr)
            continue
        res = json.loads(line[0][len("BENCH_RESULT "):])
        if out:
            with open(out, "a") as f:
                f.write(json.dumps({"build": name, "place": place, "args": args, "res": res}) + "\n")
        ph = res["phases"]["run"] if "phases" in res else res
        rows.setdefault(name, []).append(res)
        n = res["metrics"]["children"] if "metrics" in res else 1
        run = res["phases"]["run"]
        n = res["metrics"]["children"]
        keys = [k for k in run if k not in ("multiplexed", "wall_ns", "task_clock_ns", "ctx_switches", "page_faults")]
        print(name, r, f"ns/task={res['primary']['value']:.1f}", " ".join(f"{k}={run[k]/n:.1f}" for k in keys),
              "MUX" if run["multiplexed"] else "", f"cross={res['metrics']['cross_free_share']:.3f}", flush=True)

import statistics
print("medians per task")
for name, rs in rows.items():
    keys = [k for k in rs[0]["phases"]["run"] if k not in ("multiplexed",)]
    med = {k: statistics.median(r["phases"]["run"][k] / r["metrics"]["children"] for r in rs) for k in keys}
    ns = statistics.median(r["primary"]["value"] for r in rs)
    print(f"{name:6} n={len(rs)} ns/task={ns:.1f} " + " ".join(f"{k}={v:.2f}" for k, v in med.items() if k not in ("wall_ns", "task_clock_ns")))
