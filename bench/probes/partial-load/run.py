#!/usr/bin/env python3
"""Partial-load probe: one server process per (allocator, round), driven at a fixed rate by
loadgen, with the server's cores held out of deep idle (SCHED_IDLE spinners on the SMT
siblings) unless --free. Over the measured segment it runs one of:

  plain   nothing (with --raw, the request records alone)
  madvise every memory system call with its time and duration (strace -ttt -T), to line
          up with --raw's slow requests (stalls.py --calls)
  stat    perf stat --per-thread, user and kernel separately (statsum.py sums it per request)
  record  perf record -g (call graphs) into DIR/<tag>.data
  trace   perf trace -s (system call summary)
  probe   the slow-path counters of a probe-counts build (probesum.py), read from the
          server's memory at the start and end of the segment
  syscalls  strace -c of the memory system calls (madvise, mprotect, mmap, munmap, mremap, brk);
          strace slows each call it sees, so time these runs for nothing else
  sizes   allocations and frees by class (sizesum.py) of a build whose bench-alloc counts
          them into `BENCH_SIZE_COUNTS` (an `asmalloc-sizes` feature on a throwaway branch;
          docs/research/servers-h2.md says how it was built)

  run.py MODE --server NAME --allocs BUILD,... --rounds N --out DIR [--free] [--pin]
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


def probe_reader(pid, binary, symbol="ASMALLOC_PROBE_COUNTS"):
    nm = subprocess.run(["nm", "-S", str(binary)], capture_output=True, text=True).stdout
    sym = [l.split() for l in nm.splitlines() if l.endswith(" " + symbol)]
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


INPUTS = Path(os.environ.get("BENCH_APPS_DATA", "/home/bpowers/src/asmalloc/bench/apps/target")) / "inputs"
ORDER = ["--path", "/orders", "--content-type", "application/json", "--body", str(B / "apps/orders/order.json")]
LOG = ["--path", "/bench.v1.LogIngestService/Ingest"]
CONNECT = LOG + ["--content-type", "application/proto", "--header", "Connect-Protocol-Version: 1",
                 "--body", str(INPUTS / "connect/log50.bin")]
GRPC = LOG + ["--content-type", "application/grpc", "--header", "te: trailers", "--body",
              str(INPUTS / "connect/log50.grpc")]
H2 = ["--proto", "h2", "--conns", "8", "--streams", "8"]
H1 = ["--conns", "64"]
# name: (binary under bench/target/<build>/, its arguments, the generator's request arguments)
SERVERS = {
    "actix": ("apps/orders/release/orders", ["actix"], H1 + ORDER),
    "axum": ("apps/orders/release/orders", ["axum"], H1 + ORDER),
    "axum-h2": ("apps/orders/release/orders", ["axum"], H2 + ORDER),
    "connect": ("release/rpc-server", ["log"], H1 + CONNECT),
    "connect-h2": ("release/rpc-server", ["log"], H2 + CONNECT),
    "grpc-h2": ("release/rpc-server", ["log"], H2 + GRPC),
    "cache-4G": ("apps/orders/release/orders", ["axum", "keep=2250000"], H1 + ORDER),
    "cache-2G": ("apps/orders/release/orders", ["axum", "keep=1120000"], H1 + ORDER),
    "miniredis-2G": ("apps/miniredis/release/miniredis", ["serve", "preload=1800000"],
                     H1 + ["--proto", "resp", "--requests", str(INPUTS / "miniredis/churn-1800000.resp")]),
    "miniredis": ("apps/miniredis/release/miniredis", ["serve"],
                  H1 + ["--proto", "resp", "--requests", str(INPUTS / "miniredis/commands.resp")]),
}


def no_thp():
    """Turns transparent huge pages off for the server (PR_SET_THP_DISABLE, inherited across
    exec), so that a run can show what huge pages and the compaction behind them cost."""
    import ctypes
    if ctypes.CDLL(None, use_errno=True).prctl(41, 1, 0, 0, 0) != 0:
        raise OSError(ctypes.get_errno(), "prctl(PR_SET_THP_DISABLE)")


def server_bin(build, server):
    rel, sargs, _ = SERVERS[server]
    return B / "target" / build / rel, sargs


def run(mode, server, build, rnd, args, out):
    binary, sargs = server_bin(build, server)
    tag = f"{server}-{build}-{rnd}{'-free' if args.free else ''}{'-pin' if args.pin else ''}{'-nothp' if args.no_thp else ''}"
    so = open(out / f"{tag}.server.out", "w+")
    # The madvise mode starts the server under strace, which with --seccomp-bpf stops only at
    # the calls it records (attached to a running server it stops at every call, and the
    # server falls behind); the wall and monotonic clocks are written together so that the
    # calls can be lined up with the requests.
    traced = []
    if mode == "madvise":
        (out / f"{tag}.clocks").write_text(f"{time.time()} {time.monotonic_ns()}\n")
        traced = ["strace", "-f", "--seccomp-bpf", "-ttt", "-T", "-e", "trace=madvise,mprotect,mmap,munmap",
                  "-o", str(out / f"{tag}.madvise")]
    srv = subprocess.Popen(["taskset", "-c", "0-3"] + traced + [str(binary)] + sargs, stdout=so, stderr=subprocess.STDOUT,
                           preexec_fn=no_thp if args.no_thp else None,
                           env=dict({"PATH": os.environ["PATH"], "HOME": os.environ["HOME"], "BENCH_CPUS": "0-3"},
                               # tcmalloc runs its per-CPU caches only with glibc's rseq off.
                               **({"GLIBC_TUNABLES": "glibc.pthread.rseq=0"} if build == "tc" else {})))
    spin = []
    if not args.free:
        for c in (16, 17, 18, 19):
            spin.append(subprocess.Popen(["chrt", "--idle", "0", "taskset", "-c", str(c), "sh", "-c",
                                          "while :; do :; done"]))
    pid = srv.pid
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
        pid = srv.pid
        if traced:
            pid = int(Path(f"/proc/{srv.pid}/task/{srv.pid}/children").read_text().split()[0])
        if args.pin:
            # One worker per CPU, fixed, so that no worker migrates.
            tids = sorted(int(t) for t in os.listdir(f"/proc/{pid}/task")
                          if Path(f"/proc/{pid}/task/{t}/comm").read_text().startswith("actix-rt"))
            for cpu, tid in zip((0, 1, 2, 3), tids):
                subprocess.run(["taskset", "-p", "-c", str(cpu), str(tid)], stdout=subprocess.DEVNULL, check=True)
        plan = args.plan or f"_fill:closed:{args.fill},_w:{args.rate}:2,m:{args.rate}:{args.secs}"
        gen = subprocess.Popen([str(LOADGEN), "--addr", addr] + SERVERS[server][2]
                               + ["--threads", "4", "--cpus", "8-11", "--plan", plan]
                               + (["--raw", str(out / f"{tag}.raw"), "--pid", str(pid), "--mem",
                                   str(out / f"{tag}.mem")] if args.raw else []),
                               stdout=subprocess.PIPE, text=True)
        perf = None
        result = None
        reader = {"probe": lambda: probe_reader(pid, binary),
                  "sizes": lambda: probe_reader(pid, binary, "BENCH_SIZE_COUNTS")}.get(mode, lambda: None)()
        before = None
        for line in gen.stdout:
            if line.startswith("MARK") and line.split()[3:4] == ["m"]:
                if reader:
                    before = reader()
                if mode == "stat":
                    perf = subprocess.Popen(["taskset", "-c", "15", "perf", "stat", "-x,", "--per-thread", "-e",
                                             EVENTS, "-p", str(pid), "-o", str(out / f"{tag}.stat")],
                                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
                elif mode == "record":
                    perf = subprocess.Popen(["taskset", "-c", "15", "perf", "record", "-g", "-F", "4999", "-p",
                                             str(pid), "-o", str(out / f"{tag}.data")],
                                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
                elif mode == "syscalls":
                    perf = subprocess.Popen(["taskset", "-c", "15", "strace", "-c", "-f", "-e",
                                             "trace=madvise,mprotect,mmap,munmap,mremap,brk", "-p", str(pid),
                                             "-o", str(out / f"{tag}.syscalls")],
                                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
                elif mode == "trace":
                    perf = subprocess.Popen(["taskset", "-c", "15", "perf", "trace", "-s", "-p", str(pid),
                                             "-o", str(out / f"{tag}.trace")],
                                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            elif line.startswith("MARK") and line.split()[2] == "end":
                (out / f"{tag}.smaps").write_text(Path(f"/proc/{pid}/smaps_rollup").read_text())
                if perf:
                    perf.send_signal(signal.SIGINT)
                if reader and before:
                    after = reader()
                    (out / f"{tag}.{mode}.json").write_text(json.dumps({i: b - a for i, (a, b) in enumerate(zip(before, after)) if b != a}))
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
        if traced and pid != srv.pid:
            os.kill(pid, signal.SIGTERM)
        else:
            srv.send_signal(signal.SIGTERM)
        srv.wait(timeout=20)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("mode", choices=["plain", "madvise", "stat", "record", "trace", "probe", "sizes", "syscalls"])
    ap.add_argument("--server", default="actix", choices=sorted(SERVERS))
    ap.add_argument("--allocs", default="asmalloc,je")
    ap.add_argument("--rounds", type=int, default=2)
    ap.add_argument("--rate", default="280000")
    ap.add_argument("--secs", default="8")
    ap.add_argument("--fill", default="2", help="seconds of closed-loop load before the fixed rate")
    ap.add_argument("--free", action="store_true")
    ap.add_argument("--pin", action="store_true")
    ap.add_argument("--raw", action="store_true",
                    help="keep every request (loadgen --raw) in DIR/<tag>.raw and the resident set in DIR/<tag>.mem")
    ap.add_argument("--plan", help="the generator's whole plan; the measured segment must be named m")
    ap.add_argument("--no-thp", action="store_true", help="run the server with transparent huge pages off")
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
