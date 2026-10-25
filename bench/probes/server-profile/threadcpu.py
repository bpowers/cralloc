#!/usr/bin/env python3
"""CPU time of a server's threads by name over a window of its plan, beside a benchrun session:

  threadcpu.py --exe-glob GLOB --start S --end E --out FILE.jsonl

Watches /proc for processes whose executable matches GLOB (the servers of the session, one
after another) and for the load generator (bench/target/loadgen); for each server run it reads
every thread's user and system time (/proc/PID/task/TID/stat) at S and at E seconds after the
generator starts, and writes one line: the server's pid, its executable, and the CPU seconds
each thread name (digits replaced by #) spent in the window. A server whose driver runs it in
a child of the same binary is followed through the process with the most threads.
"""
import argparse
import fnmatch
import json
import os
import re
import time
from collections import defaultdict

TICK = os.sysconf("SC_CLK_TCK")


def exe(pid):
    try:
        return os.readlink(f"/proc/{pid}/exe")
    except OSError:
        return None


def threads(pid):
    out = {}
    try:
        tids = os.listdir(f"/proc/{pid}/task")
    except OSError:
        return out
    for tid in tids:
        try:
            s = open(f"/proc/{pid}/task/{tid}/stat").read()
        except OSError:
            continue
        comm = s[s.index("(") + 1:s.rindex(")")]
        f = s[s.rindex(")") + 2:].split()
        out[tid] = (comm, (int(f[11]) + int(f[12])) / TICK)
    return out


def by_name(before, after):
    tot = defaultdict(float)
    for tid, (comm, t) in after.items():
        t0 = before.get(tid, (comm, 0.0))[1]
        tot[re.sub(r"[0-9]+", "#", comm)] += t - t0
    return dict(sorted(tot.items(), key=lambda kv: -kv[1]))


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--exe-glob", required=True)
    ap.add_argument("--start", type=float, required=True)
    ap.add_argument("--end", type=float, required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--idle-exit", type=float, default=600)
    a = ap.parse_args()
    seen = set()
    last = time.time()
    while time.time() - last < a.idle_exit:
        gen = None
        servers = []
        for d in os.listdir("/proc"):
            if not d.isdigit():
                continue
            e = exe(d)
            if e is None:
                continue
            if e.endswith("/target/loadgen/release/loadgen") and d not in seen:
                gen = d
            elif fnmatch.fnmatch(e, a.exe_glob):
                servers.append(d)
        if not gen:
            time.sleep(0.05)
            continue
        seen.add(gen)
        t0 = time.monotonic()
        server = max(servers, key=lambda p: len(threads(p)), default=None)
        if server is None:
            continue
        path = exe(server)
        while time.monotonic() < t0 + a.start:
            time.sleep(0.01)
        before = threads(server)
        while time.monotonic() < t0 + a.end:
            time.sleep(0.01)
        after = threads(server)
        with open(a.out, "a") as f:
            f.write(json.dumps({"pid": int(server), "exe": path, "window": [a.start, a.end],
                                "cpu_s": by_name(before, after)}) + "\n")
        last = time.time()


if __name__ == "__main__":
    main()
