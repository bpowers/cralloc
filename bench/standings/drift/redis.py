#!/usr/bin/env python3
"""The mini-redis run of docs/research/heap-drift.md: a real server whose keys are overwritten
at changing value sizes. Run from the repository root under scripts/heavy:

    scripts/heavy python3 -I bench/probes/heap-drift/redis.py BUILD OUTFILE [--keys N]
        [--phases wide,small,...] [--rate 80000] [--quiet 60]

BUILD is a directory of bench/target with apps/miniredis/release/miniredis in it
(build-miniredis.sh). The server starts preloaded with N keys (values of 64 B to 4 KiB, about
1.1 KiB live a key), on CPUs 12-15; then, for each phase, the load generator (on 28-31) sends
the phase's file of requests.py once, at RATE commands a second, which overwrites every key
at the phase's sizes; then nothing for QUIET seconds. The server runs under an 8 GiB cap, the
generator under 4 GiB.

Every second the driver writes a JSON line to OUTFILE: the phase (its index, name and seconds
since it began), the server's anonymous resident memory, its peak resident set, its cgroup's memory.current, and with the
`live-bytes` build, the bytes the program holds (bench/alloc/src/live.rs). The program's live
bytes do not depend on the allocator, so the live-bytes build's run is the denominator for
every build's, matched by phase and second. Phases begin when the generator has read its file,
so a phase's clock, not the run's, is what runs line up on.
"""
import json
import os
import signal
import subprocess
import sys
import threading
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
SERVER_CPUS = "12-15"
LOAD_CPUS = "28-31"


def arg(name, default):
    if name in sys.argv:
        return sys.argv[sys.argv.index(name) + 1]
    return default


def status(pid):
    out = {}
    try:
        for line in Path(f"/proc/{pid}/status").read_text().splitlines():
            k, _, v = line.partition(":")
            if k in ("RssAnon", "VmHWM", "VmRSS"):
                out[k] = int(v.split()[0]) * 1024
    except OSError:
        pass
    return out


def cgroup_current(pid):
    """memory.current of the process's cgroup (the server's memlimit scope), or None."""
    try:
        line = next(l for l in Path(f"/proc/{pid}/cgroup").read_text().splitlines() if l.startswith("0::"))
        return int(Path(f"/sys/fs/cgroup{line[3:].strip()}/memory.current").read_text())
    except (OSError, StopIteration, ValueError):
        return None


def descendant(root, exe):
    """The process under `root` whose executable is `exe`."""
    children = {}
    for d in Path("/proc").iterdir():
        if not d.name.isdigit():
            continue
        try:
            ppid = int((d / "stat").read_text().rsplit(")", 1)[1].split()[1])
        except (OSError, IndexError, ValueError):
            continue
        children.setdefault(ppid, []).append(int(d.name))
    todo = [root]
    while todo:
        p = todo.pop()
        try:
            if os.path.realpath(f"/proc/{p}/exe") == str(exe):
                return p
        except OSError:
            pass
        todo.extend(children.get(p, []))
    return None


class LiveBytes:
    """bench-alloc's BENCH_LIVE_BYTES table in a live-bytes build, read through /proc/PID/mem."""

    def __init__(self, pid, exe):
        nm = subprocess.run(["nm", str(exe)], capture_output=True, text=True).stdout
        sym = next((l.split()[0] for l in nm.splitlines() if l.endswith(" BENCH_LIVE_BYTES")), None)
        self.at = None
        if sym is None:
            return
        for line in Path(f"/proc/{pid}/maps").read_text().splitlines():
            f = line.split()
            if len(f) >= 6 and f[5] == str(exe) and f[2] == "00000000":
                self.at = int(f[0].split("-")[0], 16) + int(sym, 16)
                break
        self.pid = pid

    def read(self):
        if self.at is None:
            return None
        with open(f"/proc/{self.pid}/mem", "rb") as f:
            f.seek(self.at)
            b = f.read(64 * 64)
        return sum(int.from_bytes(b[i : i + 8], "little", signed=True) for i in range(0, len(b), 64))


def main():
    build, out = sys.argv[1], Path(sys.argv[2])
    keys = int(arg("--keys", "1200000"))
    phases = arg("--phases", "wide,small,mid,small,wide,small,mid,small,wide").split(",")
    rate = int(arg("--rate", "80000"))
    quiet = float(arg("--quiet", "60"))
    exe = (ROOT / f"bench/target/{build}/apps/miniredis/release/miniredis").resolve()
    loadgen = ROOT / "bench/target/loadgen/release/loadgen"
    inputs = ROOT / "target/heap-drift/redis-inputs"
    env = dict(os.environ)
    if build == "tc":
        env["GLIBC_TUNABLES"] = "glibc.pthread.rseq=0"
    if build == "asmalloc-memstat":
        env.update(ASMALLOC_MEMSTAT=f"{out}.memstat", ASMALLOC_MEMSTAT_MS="5000", ASMALLOC_MEMSTAT_CPU="15")
    memlimit = str(ROOT / "scripts/memlimit")
    server = subprocess.Popen(
        [memlimit, "-m", "8G", "-t", "60m", "--", "taskset", "-c", SERVER_CPUS, str(exe), "serve", f"preload={keys}"],
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        env=env,
    )
    addr = None
    for line in server.stdout:
        if line.startswith("BENCH_LISTEN "):
            addr = line.split()[1]
            break
    if addr is None:
        sys.exit(f"server exited before listening: {server.wait()}")
    pid = descendant(server.pid, exe)
    live = LiveBytes(pid, exe)
    state = {"i": -1, "phase": "preloaded", "t0": time.monotonic()}
    stop = threading.Event()
    log = open(out, "w")
    log.write(json.dumps({"build": build, "keys": keys, "phases": phases, "rate": rate, "exe": str(exe)}) + "\n")
    start = time.monotonic()

    def sample():
        while not stop.is_set():
            now = time.monotonic()
            s = status(pid)
            rec = {
                "i": state["i"],
                "phase": state["phase"],
                "t_phase": round(now - state["t0"], 2),
                "t": round(now - start, 2),
                "rss_anon": s.get("RssAnon"),
                "hwm": s.get("VmHWM"),
                "cgroup": cgroup_current(pid),
                "live": live.read(),
            }
            log.write(json.dumps(rec) + "\n")
            log.flush()
            stop.wait(1.0)

    sampler = threading.Thread(target=sample)
    sampler.start()
    ok = True
    for i, name in enumerate(phases):
        requests = inputs / f"{name}-{keys}.resp"
        secs = 2 * keys / rate
        gen = subprocess.Popen(
            [memlimit, "-m", "4G", "-t", "30m", "--", str(loadgen), "--addr", addr, "--proto", "resp",
             "--requests", str(requests), "--conns", "32", "--threads", "2", "--cpus", LOAD_CPUS,
             "--plan", f"{name}:{rate}:{secs:.3f}"],
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
        )
        for line in gen.stdout:
            if line.startswith("MARK ") and line.split()[2] == "0":
                state.update(i=i, phase=name, t0=time.monotonic())
            elif line.startswith("RESULT "):
                result = json.loads(line[7:])
                log.write(json.dumps({"phase_result": i, "name": name, "result": result}) + "\n")
        if gen.wait() != 0:
            ok = False
            print(f"loadgen failed in phase {i} {name}", file=sys.stderr)
    state.update(i=len(phases), phase="quiet", t0=time.monotonic())
    time.sleep(quiet)
    stop.set()
    sampler.join()
    os.kill(pid, signal.SIGTERM)
    tail = server.stdout.read()
    code = server.wait()
    log.write(json.dumps({"server_exit": code, "ok": ok, "tail": tail[-2000:]}) + "\n")
    log.close()
    sys.exit(0 if ok and code == 0 else 1)


if __name__ == "__main__":
    main()
