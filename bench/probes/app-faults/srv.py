#!/usr/bin/env python3
"""Order services under closed-loop load, interleaved: srv.py OUT --rounds N --libs tag=path,...

Server on CPUs 0-3 under perf stat (faults, switches, cycles), oha on 8-11 (not preloaded),
400k POSTs over 64 connections. Records throughput, latency percentiles and DIAG counts.
A measurement: run under scripts/measure --both-l3 and scripts/memlimit.
"""
import os
import argparse
import json
import random
import signal
import socket
import subprocess
import time
from pathlib import Path

S = Path(os.environ.get("APPF_OUT", "/tmp/app-faults"))
R = Path(os.environ["RUST_SCREEN_ROOT"])
ORDER = Path(__file__).resolve().parents[1] / "rust-screen/servers/order.json"
SERVERS = {"axum": ("axum-orders", 18080), "actix": ("actix-orders", 18081)}
EVENTS = "cycles:u,cycles:k,task-clock,page-faults,context-switches"


def wait_port(port):
    end = time.monotonic() + 30
    while time.monotonic() < end:
        try:
            with socket.create_connection(("127.0.0.1", port), timeout=0.2):
                return
        except OSError:
            time.sleep(0.02)
    raise RuntimeError("no port")


def one(name, lib, out):
    binary, port = SERVERS[name]
    stat, err = S / "benchtmp" / "srvstat", S / "benchtmp" / "srverr"
    env = ["env"] + ([f"LD_PRELOAD={lib}"] if lib != "-" else [])
    subprocess.run([str(Path(os.environ["RUST_SCREEN_ROOT"]) / "tools/spin"), "0-3,8-11", "300"], check=True)
    p = subprocess.Popen(["perf", "stat", "-x,", "-o", str(stat), "-e", EVENTS, "--", "taskset", "-c", "0-3"]
                         + env + [str(R / "bin/servers" / binary), str(port)],
                         stdout=subprocess.DEVNULL, stderr=open(err, "w"))
    try:
        wait_port(port)
        c = subprocess.run(["taskset", "-c", "8-11", str(R / "bin/oha/oha"), "--no-tui", "--output-format", "json",
                            "-n", "400000", "-c", "64", "-m", "POST", "-T", "application/json", "-D", str(ORDER),
                            f"http://127.0.0.1:{port}/orders"], capture_output=True, text=True, timeout=300)
        o = json.loads(c.stdout)
    finally:
        kids = Path(f"/proc/{p.pid}/task/{p.pid}/children").read_text().split()
        # perf -> taskset execs env -> server: the child of perf is the server.
        for k in kids:
            subprocess.run(["kill", "-INT", k])
        p.wait(timeout=30)
    vals = {}
    for line in stat.read_text().splitlines():
        f = line.split(",")
        if len(f) > 2 and not line.startswith("#"):
            try:
                vals[f[2]] = float(f[0])
            except ValueError:
                pass
    calls = {}
    for line in err.read_text(errors="replace").splitlines():
        f = line.split()
        if len(f) >= 9 and f[:2] == ["DIAG", "site"] and not f[2].startswith("walk"):
            k = f[2].rstrip("0123456789")
            calls[k] = calls.get(k, 0) + int(f[5])
    lat = o.get("latencyPercentiles", {})
    rec = {"server": name, "lib": out, "rps": o["summary"]["requestsPerSec"], "p50": lat.get("p50"),
           "p99": lat.get("p99"), "p99.9": lat.get("p99.9"), "max": o["summary"].get("slowest"),
           "stat": vals, "calls": calls}
    return rec


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("out")
    ap.add_argument("--rounds", type=int, default=5)
    ap.add_argument("--libs", required=True)
    a = ap.parse_args()
    libs = dict(x.split("=", 1) for x in a.libs.split(","))
    (S / "benchtmp").mkdir(exist_ok=True)
    for r in range(a.rounds):
        for name in SERVERS:
            order = list(libs)
            random.shuffle(order)
            for tag in order:
                rec = one(name, libs[tag], tag)
                rec["round"] = r
                with open(a.out, "a") as f:
                    f.write(json.dumps(rec) + "\n")
                st = rec["stat"]
                print(f"{name:6} {tag:8} r{r} rps {rec['rps']:.0f} p99 {rec['p99'] * 1e3:.2f}ms p99.9 {rec['p99.9'] * 1e3:.2f}ms "
                      f"max {rec['max'] * 1e3:.1f}ms kern {(st.get('cycles:k') or 0) / 1e9:.2f}G pf {int(st.get('page-faults') or 0)} "
                      f"cs {int(st.get('context-switches') or 0)} {rec['calls']}", flush=True)


if __name__ == "__main__":
    main()
