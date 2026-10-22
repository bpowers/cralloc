#!/usr/bin/env python3
"""connect log grpc h2 by hand: rpc-server (log) on CPUs 0-3, loadgen on 8-11 at the job's
s70 rate, and `perf stat -p` on the server over six seconds of the fixed rate. Builds are
interleaved in a new random order each round. Run under scripts/measure --both-l3.

usage: grpc-pstat.py ROUNDS EVENTS BUILD...
  BUILD   a directory under bench/target holding release/rpc-server (asm-main, ...)
  EVENTS  a perf event list; at most five (the NMI watchdog holds a counter), or it is multiplexed
  env: PERF_RECORD=1 records cycles samples instead (-o bench/target/perf/<build>-<round>.data)
Prints, per run, each event per request.
"""
import os, random, signal, subprocess, sys, time

WT = str(__import__("pathlib").Path(__file__).resolve().parents[3])
APPS = subprocess.run(["git", "-C", WT, "rev-parse", "--path-format=absolute", "--git-common-dir"],
                      capture_output=True, text=True).stdout.strip() + "/../bench/apps/target/inputs/connect"
rounds, events, builds = int(sys.argv[1]), sys.argv[2], sys.argv[3:]
RATE, SECS = 63000, 6
# The job's plan up to and through its s70 segment (suite.rs, `day`), so the server is in the
# state benchrun measures it in; perf counts the s70 segment.
PLAN = f"_fill:closed:2,sat:closed:4,_w30:27000:1,s30:27000:6,_w70:{RATE}:1,s70:{RATE}:{SECS + 1}"
BEFORE = 2 + 4 + 1 + 6 + 1


def run(build, rnd):
    binp = f"{WT}/bench/target/{build}/release/rpc-server"
    env = {k: v for k, v in os.environ.items() if k in ("PATH", "HOME", "USER", "LANG")}
    env["BENCH_CPUS"] = "0-3"
    srv = subprocess.Popen(["taskset", "-c", "0-3", binp, "log"], env=env, stdout=subprocess.PIPE,
                           stderr=subprocess.DEVNULL, text=True)
    addr = None
    for line in srv.stdout:
        if line.startswith("BENCH_LISTEN"):
            addr = line.split()[1]
            break
    plan = PLAN
    lg = [f"{WT}/bench/target/loadgen/release/loadgen", "--addr", addr, "--proto", "h2", "--streams", "8",
          "--path", "/bench.v1.LogIngestService/Ingest", "--content-type", "application/grpc",
          "--header", "te: trailers", "--body", f"{APPS}/log50.grpc", "--conns", "8", "--threads", "4",
          "--cpus", "8-11", "--plan", plan, "--label", "x"]
    g = subprocess.Popen(lg, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    time.sleep(BEFORE)
    out = f"{WT}/bench/target/perf/{build}-{rnd}"
    os.makedirs(os.path.dirname(out), exist_ok=True)
    if os.environ.get("PERF_RECORD"):
        cmd = ["perf", "record", "-q", "-o", out + ".data", "-e", events, "-p", str(srv.pid), "--", "sleep", str(SECS)]
    else:
        cmd = ["perf", "stat", "-x", ",", "-o", out + ".stat", "-e", events, "-p", str(srv.pid), "--", "sleep", str(SECS)]
    subprocess.run(cmd, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    g.wait(timeout=120)
    srv.send_signal(signal.SIGINT)
    srv.wait(timeout=60)
    if os.environ.get("PERF_RECORD"):
        print(build, rnd, "recorded", out + ".data", flush=True)
        return
    req = RATE * SECS
    vals = {}
    for line in open(out + ".stat"):
        f = line.strip().split(",")
        if len(f) > 3 and f[0] and f[0][0].isdigit():
            vals[f[2]] = (float(f[0]), f[4] if len(f) > 4 else "")
    mux = any(p and p not in ("100.00", "") for _, p in vals.values())
    print(build, rnd, " ".join(f"{k}={v / req:.1f}" for k, (v, _) in vals.items()), "MUX" if mux else "", flush=True)


for r in range(rounds):
    order = builds[:]
    random.shuffle(order)
    for b in order:
        run(b, r)
