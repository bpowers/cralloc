#!/usr/bin/env python3
"""One drift workload run (bench/workloads/src/drift.rs) for docs/research/large-heap-constants.md,
with a watcher beside it. Run from the repository root under scripts/heavy (a memory run; not
timed):

    scripts/heavy python3 -I bench/probes/large-heap-constants/drift.py BUILD OUT [--mem 16G]
        [--period 0.25] [--env K=V ...] -- key=value ...

BUILD is a directory of bench/target with release/workload in it. The workload's output goes to
OUT (its DRIFT lines and BENCH_RESULT), and every PERIOD seconds a sample of the process
(watchlib.py: resident, page tables, mappings, faults, the background thread's CPU time and
wake-ups, and a diagnostic build's counters) goes to OUT.watch.jsonl, one JSON object a line.
The first line of each file says what ran. The cap is MEM (never above 16G).
"""
import json
import os
import subprocess
import sys
import threading
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.dont_write_bytecode = True
sys.path.insert(0, str(HERE))
import watchlib  # noqa: E402

ROOT = HERE.parents[2]


def main():
    argv = sys.argv[1:]
    if "--" not in argv:
        sys.exit(__doc__)
    split = argv.index("--")
    opts, args = argv[:split], argv[split + 1:]
    build, out = opts[0], Path(opts[1])
    mem, period, env_extra = "16G", 0.25, {}
    i = 2
    while i < len(opts):
        if opts[i] == "--mem":
            mem = opts[i + 1]
        elif opts[i] == "--period":
            period = float(opts[i + 1])
        elif opts[i] == "--env":
            k, _, v = opts[i + 1].partition("=")
            env_extra[k] = v
        else:
            sys.exit(f"unknown option {opts[i]}")
        i += 2
    assert mem.endswith("G") and int(mem[:-1]) <= 16, "the cap is never above 16G"
    exe = (ROOT / f"bench/target/{build}/release/workload").resolve()
    if not exe.exists():
        sys.exit(f"no workload at {exe}")
    env = dict(os.environ)
    if build == "tc" or build.startswith("tc-"):
        env["GLIBC_TUNABLES"] = "glibc.pthread.rseq=0"
    env.update(env_extra)
    out.parent.mkdir(parents=True, exist_ok=True)
    head = {
        "build": build,
        "exe": str(exe),
        "built_from": (exe.parents[1] / "BUILT-FROM").read_text().strip() if (exe.parents[1] / "BUILT-FROM").exists() else None,
        "args": args,
        "env": env_extra,
        "mem": mem,
        "commit": subprocess.run(["git", "-C", str(ROOT), "rev-parse", "--short", "HEAD"], capture_output=True, text=True).stdout.strip(),
        "start": time.strftime("%Y-%m-%dT%H:%M:%S%z"),
    }
    log = open(out, "w")
    log.write("# " + json.dumps(head) + "\n")
    log.flush()
    watch_file = open(str(out) + ".watch.jsonl", "w")
    watch_file.write(json.dumps({"head": head, "period": period}) + "\n")
    t0 = time.monotonic()
    proc = subprocess.Popen(
        [str(ROOT / "scripts/memlimit"), "-m", mem, "-t", "60m", "--", str(exe), "drift", *args],
        stdout=log,
        stderr=subprocess.STDOUT,
        env=env,
    )
    pid = None
    for _ in range(200):
        pid = watchlib.descendant(proc.pid, exe)
        if pid is not None or proc.poll() is not None:
            break
        time.sleep(0.01)
    stop = threading.Event()

    def sampler():
        if pid is None:
            return
        w = watchlib.Watch(pid, exe, t0)
        while not stop.is_set():
            rec = w.sample(time.monotonic())
            if rec is None:
                break
            watch_file.write(json.dumps(rec, separators=(",", ":")) + "\n")
            stop.wait(period)

    th = threading.Thread(target=sampler)
    th.start()
    code = proc.wait()
    stop.set()
    th.join()
    watch_file.close()
    log.write(f"# exit {code} {time.strftime('%Y-%m-%dT%H:%M:%S%z')}\n")
    log.close()
    sys.exit(code)


if __name__ == "__main__":
    main()
