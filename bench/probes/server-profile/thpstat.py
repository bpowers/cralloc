#!/usr/bin/env python3
"""Huge-page events of a session's servers, beside a benchrun session:

  thpstat.py --exe-glob GLOB --out FILE.jsonl [--period 1]

For each process whose executable matches GLOB (a driver that runs the server in a child of
the same binary gives two, in one cgroup), reads the `thp_*` counters of its cgroup's
`memory.stat` every PERIOD seconds while the process lives, and writes one line per process
when it is gone: its pid, start, cgroup, and the counters' first and last readings, with the
machine's `/proc/vmstat` `thp_*` counters at the same two moments (system-wide), and
`series`: `[seconds since start, thp_collapse_alloc]` at each reading where a counter moved.
khugepaged's collapses are charged to the cgroup of the process whose memory it collapsed,
so `thp_collapse_alloc` there is this server's.
"""
import argparse
import fnmatch
import json
import os
import signal
import time


def exe(pid):
    try:
        return os.readlink(f"/proc/{pid}/exe")
    except OSError:
        return None


def stat(path, prefix="thp_"):
    out = {}
    try:
        for line in open(path):
            k, v = line.split()
            if k.startswith(prefix):
                out[k] = int(v)
    except OSError:
        return None
    return out


def cgroup(pid):
    try:
        line = open(f"/proc/{pid}/cgroup").read().strip().split("\n")[0]
        return "/sys/fs/cgroup" + line.split("::", 1)[1]
    except (OSError, IndexError):
        return None


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--exe-glob", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--period", type=float, default=1.0)
    ap.add_argument("--idle-exit", type=float, default=600)
    a = ap.parse_args()
    live = {}
    last_seen = time.time()

    def flush(*_):
        # A session stops the poller as soon as its last server exits, often before the poll
        # that would see the exit; that server's record is written here.
        with open(a.out, "a") as f:
            for r in live.values():
                f.write(json.dumps(r) + "\n")
        raise SystemExit(0)

    signal.signal(signal.SIGTERM, flush)
    signal.signal(signal.SIGINT, flush)
    while time.time() - last_seen < a.idle_exit:
        for d in os.listdir("/proc"):
            if d.isdigit() and d not in live:
                e = exe(d)
                if e and fnmatch.fnmatch(e, a.exe_glob):
                    cg = cgroup(d)
                    live[d] = {"pid": int(d), "exe": e, "start": time.time(), "cgroup": cg,
                               "first": stat(f"{cg}/memory.stat") if cg else None,
                               "vm_first": stat("/proc/vmstat"), "last": None, "vm_last": None}
        for d, r in list(live.items()):
            if os.path.exists(f"/proc/{d}"):
                s = stat(f"{r['cgroup']}/memory.stat") if r["cgroup"] else None
                if s is not None:
                    if s != r["last"]:
                        r.setdefault("series", []).append([round(time.time() - r["start"], 1), s.get("thp_collapse_alloc", 0)])
                    r["last"] = s
                    r["vm_last"] = stat("/proc/vmstat")
                last_seen = time.time()
            else:
                with open(a.out, "a") as f:
                    f.write(json.dumps(r) + "\n")
                del live[d]
        time.sleep(a.period)


if __name__ == "__main__":
    main()
