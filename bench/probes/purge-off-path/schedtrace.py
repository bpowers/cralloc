#!/usr/bin/env python3
"""Run-queue delay of a server's threads, beside a servers-thread session
(docs/research/purge-off-path.md): every 2 ms, each thread's CPU time and run-queue wait from
/proc/PID/task/TID/schedstat, and its name, for the newest process whose executable is one of
--exe, while one runs. Writes one JSON line a sample; `schedsum.py` reads them.

  schedtrace.py --out FILE --exe PATH [--exe PATH...]

Run pinned to a CPU the server and the load do not use (`taskset -c 15`), as default-thread's
watcher is; stop it with SIGTERM.
"""
import argparse
import json
import os
import signal
import time

stop = False


def on_term(*_):
    global stop
    stop = True


def find(exes):
    best = None
    for pid in os.listdir("/proc"):
        if not pid.isdigit():
            continue
        try:
            exe = os.readlink(f"/proc/{pid}/exe")
        except OSError:
            continue
        if exe in exes and (best is None or int(pid) > best):
            best = int(pid)
    return best


def sample(pid):
    out = {}
    for tid in os.listdir(f"/proc/{pid}/task"):
        try:
            run, wait, _ = open(f"/proc/{pid}/task/{tid}/schedstat").read().split()
            name = open(f"/proc/{pid}/task/{tid}/comm").read().strip()
        except OSError:
            continue
        out[tid] = [name, int(run), int(wait)]
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", required=True)
    ap.add_argument("--exe", action="append", required=True)
    a = ap.parse_args()
    exes = {os.path.realpath(e) for e in a.exe}
    signal.signal(signal.SIGTERM, on_term)
    with open(a.out, "w") as f:
        pid = None
        while not stop:
            if pid is None or not os.path.exists(f"/proc/{pid}"):
                pid = find(exes)
                if pid is None:
                    time.sleep(0.05)
                    continue
            try:
                s = sample(pid)
            except OSError:
                pid = None
                continue
            exe = os.path.realpath(f"/proc/{pid}/exe")
            f.write(json.dumps({"t": time.monotonic(), "pid": pid, "exe": exe, "tasks": s}) + "\n")
            time.sleep(0.002)


main()
