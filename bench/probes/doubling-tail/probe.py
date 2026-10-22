#!/usr/bin/env python3
"""The order service's table doubling (`orders axum doubling 4G`), one server per run, with a
tracer for the doublings: the suite's plan and placement (server on CPUs 0-3 with SMT spinners,
loadgen on 8-11), and the server's record of every rehash (start, wall and CPU time, faults,
context switches, thread; `REHASH` lines on its stderr).

  probe.py MODE --builds asmalloc,je --rounds N --out DIR [--window 11:9] [--period 100000]

MODE:
  plain   the server's records, a 5 ms sampler of the machine's vmstat counters, and the
          server's memory map at the end
  perf    the same, with perf record of the server (cycles every --period with kernel call
          chains, context switches with call chains) on CLOCK_MONOTONIC, over --window
          START:SECS of segment dbl; the doublings come about 13 s into it

A BUILD is a directory under bench/target/ holding apps/orders (`benchrun build-apps`, or a
copy of one built by hand with a patched allocator). A build whose binary has the symbol
ASMALLOC_PROBE_EVENTS (a ring of timed allocator events, from a temporary patch kept out of
the tree; docs/research/doubling-tail.md) has the ring saved too. analyze.py reads DIR.

Run it under scripts/measure --both-l3, with --out on disk (/tmp is memory). The server runs
under scripts/memlimit, and a run whose server has a finite address-space limit is refused:
asmalloc would size its regions to it.
"""
import argparse
import os
import random
import signal
import subprocess
import threading
import time
from pathlib import Path

W = Path(__file__).resolve().parents[3]
B = W / "bench"
LOADGEN = B / "target/loadgen/release/loadgen"
MEMLIMIT = W / "scripts/memlimit"
ORDER = ["--path", "/orders", "--content-type", "application/json", "--body", str(B / "apps/orders/order.json")]
RATE = 248000
PLAN = f"_fill:{RATE}:44,dbl:{RATE}:24,idle:close:2"


def vmstat_sampler(path, stop):
    keys = ("compact_stall", "compact_fail", "compact_success", "allocstall_normal", "allocstall_movable",
            "pgscan_direct", "thp_fault_alloc", "thp_fault_fallback", "pgfault", "nr_free_pages",
            "tlb_remote_flush", "tlb_remote_flush_received")
    with open(path, "w") as out:
        while not stop.is_set():
            t = time.monotonic_ns()
            vals = {}
            for line in open("/proc/vmstat"):
                k, v = line.split()
                if k in keys:
                    vals[k] = v
            out.write(f"{t} " + " ".join(f"{k}={vals.get(k, '')}" for k in keys) + "\n")
            time.sleep(0.005)


def read_probe(pid, binary, path):
    """Saves the probe build's ring of timed allocator events (a temporary patch's
    ASMALLOC_PROBE_EVENTS), if the binary has one."""
    nm = subprocess.run(["nm", "-S", str(binary)], capture_output=True, text=True).stdout
    sym = [l.split() for l in nm.splitlines() if l.endswith(" ASMALLOC_PROBE_EVENTS")]
    if not sym:
        return
    value, size = int(sym[0][0], 16), int(sym[0][1], 16)
    real = os.path.realpath(binary)
    base = None
    for line in Path(f"/proc/{pid}/maps").read_text().splitlines():
        f = line.split()
        if len(f) >= 6 and f[5] == real and int(f[2], 16) == 0:
            base = int(f[0].split("-")[0], 16)
            break
    with open(f"/proc/{pid}/mem", "rb") as m:
        m.seek(base + value)
        Path(path).write_bytes(m.read(size))


def run(mode, build, rnd, args, out):
    binary = B / "target" / build / "apps/orders/release/orders"
    tag = f"{build}-{rnd}"
    so = open(out / f"{tag}.server.out", "w+")
    se = open(out / f"{tag}.server.err", "w")
    (out / f"{tag}.clocks").write_text(f"{time.time_ns()} {time.monotonic_ns()}\n")
    # memlimit needs the session's environment to reach systemd (without it, it falls back to
    # an address-space limit, under which asmalloc shrinks its regions); the server gets only
    # what the suite gives it.
    srv = subprocess.Popen([str(MEMLIMIT), "-m", "12G", "-t", "10m", "--", "env", "-i", f"PATH={os.environ['PATH']}",
                            f"HOME={os.environ['HOME']}", "BENCH_CPUS=0-3", "taskset", "-c", "0-3", str(binary), "axum", "keep=2250000"], stdout=so, stderr=se,
                           env=dict(os.environ, MEMLIMIT_QUIET="1"))
    spin = [subprocess.Popen(["chrt", "--idle", "0", "taskset", "-c", str(c), "sh", "-c", "while :; do :; done"])
            for c in (16, 17, 18, 19)]
    stop = threading.Event()
    sampler = threading.Thread(target=vmstat_sampler, args=(out / f"{tag}.vmstat", stop), daemon=True)
    perf = None
    try:
        addr = None
        deadline = time.time() + 120
        while time.time() < deadline and not addr:
            so.seek(0)
            for line in so.read().splitlines():
                if line.startswith("BENCH_LISTEN"):
                    addr = line.split()[1]
            time.sleep(0.02)
        if not addr:
            raise RuntimeError("server did not listen")
        # The server process itself, below memlimit and taskset.
        pid = int(subprocess.run(["pgrep", "-n", "-f", f"{binary} axum"], capture_output=True, text=True).stdout.split()[0])
        limits = Path(f"/proc/{pid}/limits").read_text()
        if not any(l.startswith("Max address space") and l.split()[3] == "unlimited" for l in limits.splitlines()):
            raise RuntimeError("the server has a finite address-space limit; asmalloc would shrink its regions")
        gen = subprocess.Popen([str(MEMLIMIT), "-m", "3G", "-t", "10m", "--", str(LOADGEN), "--addr", addr] + ORDER
                               + ["--conns", "64", "--threads", "4", "--cpus", "8-11", "--plan", PLAN],
                               stdout=subprocess.PIPE, text=True, env=dict(os.environ, MEMLIMIT_QUIET="1"))
        start, secs = (int(x) for x in args.window.split(":"))
        timer = None
        for line in gen.stdout:
            f = line.split()
            if line.startswith("MARK") and f[3:4] == ["dbl"]:
                sampler.start()
                if mode == "perf":
                    def go():
                        nonlocal perf
                        perf = subprocess.Popen(["taskset", "-c", "15", "perf", "record", "-k", "CLOCK_MONOTONIC",
                                                 "-g", "-e", f"cycles/period={args.period}/",
                                                 "-e", "context-switches/period=1/", "-p", str(pid),
                                                 "-o", str(out / f"{tag}.perf.data"), "--", "sleep", str(secs)],
                                                stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
                    timer = threading.Timer(start, go)
                    timer.start()
            elif line.startswith("RESULT"):
                (out / f"{tag}.result.json").write_text(line[7:].strip())
        gen.wait()
        if timer:
            timer.cancel()
        if perf:
            perf.wait(timeout=120)
        read_probe(pid, binary, out / f"{tag}.events")
        (out / f"{tag}.maps").write_text(Path(f"/proc/{pid}/maps").read_text())
        os.kill(pid, signal.SIGTERM)
        srv.wait(timeout=60)
        print(f"{tag}: gen exit {gen.returncode}, server exit {srv.returncode}", flush=True)
    finally:
        stop.set()
        for p in spin:
            p.kill()
            p.wait()
        if srv.poll() is None:
            srv.kill()


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("mode", choices=["plain", "perf"])
    ap.add_argument("--builds", default="asmalloc,je")
    ap.add_argument("--rounds", type=int, default=1)
    ap.add_argument("--first-round", type=int, default=1)
    ap.add_argument("--window", default="11:9")
    ap.add_argument("--period", type=int, default=100000)
    ap.add_argument("--out", required=True)
    args = ap.parse_args()
    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    for rnd in range(args.first_round, args.first_round + args.rounds):
        builds = args.builds.split(",")
        random.shuffle(builds)
        for b in builds:
            run(args.mode, b, rnd, args, out)


if __name__ == "__main__":
    main()
