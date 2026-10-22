#!/usr/bin/env python3
"""Watches the servers a benchrun session starts, for docs/research/default-thread-servers.md:
every PERIOD seconds, for each live process whose executable is one of EXE, the voluntary and
involuntary context switches and the CPU time of its threads named `asmalloc` (the default
background thread), its resident set, and its cgroup's memory and anonymous memory. It also
notes when each load generator (LOADGEN) starts, which is where the plan's clock begins.

  watch.py --out FILE --exe PATH [--exe PATH...] --loadgen PATH [--period 0.25]

One JSON object a line: {"type": "proc", ...} when a server is first seen, {"type": "gen", ...}
when a generator is, and {"type": "s", ...} for each sample. Times are wall-clock seconds.
It runs until it is sent SIGTERM or SIGINT. Start it beside the session, inside the same
scripts/measure, on the runner's CPU.
"""
import argparse
import json
import os
import signal
import time
from pathlib import Path

TICK = os.sysconf("SC_CLK_TCK")
PAGE = os.sysconf("SC_PAGE_SIZE")
BOOT = next(int(l.split()[1]) for l in Path("/proc/stat").read_text().splitlines() if l.startswith("btime"))


def started(pid):
    """The process's start in wall-clock seconds (to the clock tick)."""
    stat = Path(f"/proc/{pid}/stat").read_text()
    fields = stat[stat.rindex(")") + 2:].split()
    return BOOT + int(fields[19]) / TICK


def cpu_ticks(pid, tid):
    stat = Path(f"/proc/{pid}/task/{tid}/stat").read_text()
    fields = stat[stat.rindex(")") + 2:].split()
    return int(fields[11]) + int(fields[12])


def switches(pid, tid):
    vol = nonvol = None
    for line in Path(f"/proc/{pid}/task/{tid}/status").read_text().splitlines():
        if line.startswith("voluntary_ctxt_switches:"):
            vol = int(line.split()[1])
        elif line.startswith("nonvoluntary_ctxt_switches:"):
            nonvol = int(line.split()[1])
    return vol, nonvol


def cgroup_dir(pid):
    for line in Path(f"/proc/{pid}/cgroup").read_text().splitlines():
        if line.startswith("0::"):
            return Path("/sys/fs/cgroup") / line[3:].lstrip("/")
    return None


def anon(cg):
    for line in (cg / "memory.stat").read_text().splitlines():
        if line.startswith("anon "):
            return int(line.split()[1])
    return None


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", required=True)
    ap.add_argument("--exe", action="append", required=True)
    ap.add_argument("--loadgen", required=True)
    ap.add_argument("--period", type=float, default=0.25)
    a = ap.parse_args()
    servers = {os.path.realpath(e) for e in a.exe}
    loadgen = os.path.realpath(a.loadgen)
    stop = []
    signal.signal(signal.SIGTERM, lambda *_: stop.append(1))
    signal.signal(signal.SIGINT, lambda *_: stop.append(1))
    out = open(a.out, "a", buffering=1)
    seen = set()
    tracked = {}
    while not stop:
        t_next = time.monotonic() + a.period
        for d in os.listdir("/proc"):
            if not d.isdigit() or d in seen:
                continue
            pid = int(d)
            try:
                exe = os.readlink(f"/proc/{pid}/exe")
            except OSError:
                continue
            # The runner forks and moves the child into its cgroup before it execs the
            # server, so a process seen as the runner may yet become one: look again.
            if not os.path.basename(exe).startswith("benchrun"):
                seen.add(d)
            try:
                if exe in servers:
                    env = Path(f"/proc/{pid}/environ").read_bytes().split(b"\0")
                    bg = next((e.split(b"=", 1)[1].decode() for e in env if e.startswith(b"ASMALLOC_BACKGROUND=")), None)
                    cg = cgroup_dir(pid)
                    tracked[pid] = {"cg": cg, "asm": set(), "comm_seen": set()}
                    out.write(json.dumps({"type": "proc", "pid": pid, "exe": exe, "start": started(pid),
                                          "background_env": bg, "cgroup": str(cg)}) + "\n")
                elif exe == loadgen:
                    out.write(json.dumps({"type": "gen", "pid": pid, "start": started(pid)}) + "\n")
            except OSError:
                continue
        for pid, st in list(tracked.items()):
            now = time.time()
            try:
                rss = int(Path(f"/proc/{pid}/statm").read_text().split()[1]) * PAGE
                for tid in os.listdir(f"/proc/{pid}/task"):
                    if tid not in st["comm_seen"]:
                        st["comm_seen"].add(tid)
                        if Path(f"/proc/{pid}/task/{tid}/comm").read_text().strip() == "asmalloc":
                            st["asm"].add(tid)
                threads = {}
                for tid in st["asm"]:
                    try:
                        vol, nonvol = switches(pid, tid)
                        threads[tid] = [vol, nonvol, cpu_ticks(pid, tid)]
                    except OSError:
                        pass
                cg = st["cg"]
                cur = int((cg / "memory.current").read_text()) if cg else None
                an = anon(cg) if cg else None
            except OSError:
                del tracked[pid]
                out.write(json.dumps({"type": "gone", "pid": pid, "t": now}) + "\n")
                continue
            out.write(json.dumps({"type": "s", "pid": pid, "t": round(now, 4), "rss": rss, "cg_current": cur,
                                  "cg_anon": an, "threads": threads}) + "\n")
        time.sleep(max(0.0, t_next - time.monotonic()))


if __name__ == "__main__":
    main()
