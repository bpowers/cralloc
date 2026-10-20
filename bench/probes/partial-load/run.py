#!/usr/bin/env python3
"""Partial-load probe: one server process per (allocator, round), driven at a fixed rate by
loadgen, with the server's cores held out of deep idle (SCHED_IDLE spinners on the SMT
siblings) unless --free. Over the measured segment it runs one of:

  stat    perf stat --per-thread, user and kernel separately (statsum.py sums it per request)
  record  perf record -g (call graphs) into DIR/<tag>.data
  trace   perf trace -s (system call summary)
  probe   the slow-path counters of a probe-counts build (probesum.py), read from the
          server's memory at the start and end of the segment

  run.py MODE --server actix|axum --allocs BUILD,... --rounds N --out DIR [--free] [--pin]
         [--rate 280000] [--secs 8]

A BUILD is a directory under bench/target/ holding apps/orders (from `benchrun build-apps`,
or built by hand with another feature, as docs/research/servers-partial-load.md describes).
--pin fixes each actix worker to one CPU. Run it under scripts/measure --both-l3, and give
--out a directory on disk: /tmp is memory on the reference machine.
"""
import argparse
import json
import struct
import os
import random
import signal
import subprocess
import time
from pathlib import Path

W = Path(__file__).resolve().parents[3]
B = W / "bench"
LOADGEN = B / "target/loadgen/release/loadgen"
EVENTS = ("cycles:u,cycles:k,instructions:u,instructions:k,task-clock,context-switches,"
          "cpu-migrations,page-faults")


def probe_reader(pid, binary):
    nm = subprocess.run(["nm", "-S", str(binary)], capture_output=True, text=True).stdout
    sym = [l.split() for l in nm.splitlines() if l.endswith(" ASMALLOC_PROBE_COUNTS")]
    value, size = int(sym[0][0], 16), int(sym[0][1], 16)
    real = os.path.realpath(binary)
    base = None
    for line in Path(f"/proc/{pid}/maps").read_text().splitlines():
        f = line.split()
        if len(f) >= 6 and f[5] == real and int(f[2], 16) == 0:
            base = int(f[0].split("-")[0], 16)
            break

    def read():
        with open(f"/proc/{pid}/mem", "rb") as m:
            m.seek(base + value)
            return struct.unpack(f"<{size // 8}Q", m.read(size))
    return read


def server_bin(build, server):
    return B / "target" / build / "apps/orders/release/orders", [server]


def run(mode, server, build, rnd, args, out):
    binary, sargs = server_bin(build, server)
    tag = f"{server}-{build}-{rnd}{'-free' if args.free else ''}{'-pin' if args.pin else ''}"
    so = open(out / f"{tag}.server.out", "w+")
    srv = subprocess.Popen(["taskset", "-c", "0-3", str(binary)] + sargs, stdout=so, stderr=subprocess.STDOUT,
                           env={"PATH": os.environ["PATH"], "HOME": os.environ["HOME"]})
    spin = []
    if not args.free:
        for c in (16, 17, 18, 19):
            spin.append(subprocess.Popen(["chrt", "--idle", "0", "taskset", "-c", str(c), "sh", "-c",
                                          "while :; do :; done"]))
    try:
        addr = None
        for _ in range(400):
            so.seek(0)
            for line in so.read().splitlines():
                if line.startswith("BENCH_LISTEN"):
                    addr = line.split()[1]
            if addr:
                break
            time.sleep(0.02)
        if args.pin:
            # One worker per CPU, fixed, so that no worker migrates.
            tids = sorted(int(t) for t in os.listdir(f"/proc/{srv.pid}/task")
                          if Path(f"/proc/{srv.pid}/task/{t}/comm").read_text().startswith("actix-rt"))
            for cpu, tid in zip((0, 1, 2, 3), tids):
                subprocess.run(["taskset", "-p", "-c", str(cpu), str(tid)], stdout=subprocess.DEVNULL, check=True)
        plan = f"_fill:closed:2,_w:{args.rate}:2,m:{args.rate}:{args.secs}"
        gen = subprocess.Popen([str(LOADGEN), "--addr", addr, "--path", "/orders", "--content-type",
                                "application/json", "--body", str(B / "apps/orders/order.json"), "--conns", "64",
                                "--threads", "4", "--cpus", "8-11", "--plan", plan],
                               stdout=subprocess.PIPE, text=True)
        perf = None
        result = None
        reader = probe_reader(srv.pid, binary) if mode == "probe" else None
        before = None
        for line in gen.stdout:
            if line.startswith("MARK") and line.split()[3:4] == ["m"]:
                if reader:
                    before = reader()
                if mode == "stat":
                    perf = subprocess.Popen(["taskset", "-c", "15", "perf", "stat", "-x,", "--per-thread", "-e",
                                             EVENTS, "-p", str(srv.pid), "-o", str(out / f"{tag}.stat")],
                                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
                elif mode == "record":
                    perf = subprocess.Popen(["taskset", "-c", "15", "perf", "record", "-g", "-F", "4999", "-p",
                                             str(srv.pid), "-o", str(out / f"{tag}.data")],
                                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
                elif mode == "trace":
                    perf = subprocess.Popen(["taskset", "-c", "15", "perf", "trace", "-s", "-p", str(srv.pid),
                                             "-o", str(out / f"{tag}.trace")],
                                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            elif line.startswith("MARK") and line.split()[2] == "end":
                if perf:
                    perf.send_signal(signal.SIGINT)
                if reader and before:
                    after = reader()
                    (out / f"{tag}.probe.json").write_text(json.dumps({i: b - a for i, (a, b) in enumerate(zip(before, after)) if b != a}))
            elif line.startswith("RESULT"):
                result = line[7:].strip()
        gen.wait()
        if perf:
            perf.wait(timeout=60)
        (out / f"{tag}.result.json").write_text(result or "")
        print(f"{tag}: gen exit {gen.returncode}", flush=True)
    finally:
        for p in spin:
            p.kill()
            p.wait()
        srv.send_signal(signal.SIGTERM)
        srv.wait(timeout=20)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("mode", choices=["stat", "record", "trace", "probe"])
    ap.add_argument("--server", default="actix")
    ap.add_argument("--allocs", default="asmalloc,je")
    ap.add_argument("--rounds", type=int, default=2)
    ap.add_argument("--rate", default="280000")
    ap.add_argument("--secs", default="8")
    ap.add_argument("--free", action="store_true")
    ap.add_argument("--pin", action="store_true")
    ap.add_argument("--out", required=True)
    args = ap.parse_args()
    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    rng = random.Random(1)
    for rnd in range(args.rounds):
        order = args.allocs.split(",")
        rng.shuffle(order)
        for a in order:
            run(args.mode, args.server, a, rnd, args, out)
            time.sleep(1)


if __name__ == "__main__":
    main()
