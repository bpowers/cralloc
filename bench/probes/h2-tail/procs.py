#!/usr/bin/env python3
"""procs.py OUT DUR PID: samples a server's system-call and scheduling counters every 100 ms
for DUR seconds into OUT as JSON lines, with CLOCK_MONOTONIC:
  io: /proc/PID/io's syscr and syscw (read- and write-type system calls; epoll_wait,
      recvfrom and sendto count as reads and writes there), rchar and wchar;
  sched: the sum over threads of /proc/PID/task/*/schedstat (ns on CPU, ns waiting, times
      scheduled in), and the thread count.
"""
import json
import os
import sys
import time

out, dur, pid = sys.argv[1], float(sys.argv[2]), sys.argv[3]
end = time.monotonic() + dur
with open(out, "w") as f:
    while time.monotonic() < end:
        t = time.monotonic_ns()
        try:
            io = dict(l.split(": ") for l in open(f"/proc/{pid}/io").read().split("\n") if l)
            run = wait = slices = n = 0
            for tid in os.listdir(f"/proc/{pid}/task"):
                try:
                    a, b, c = open(f"/proc/{pid}/task/{tid}/schedstat").read().split()
                except OSError:
                    continue
                run, wait, slices, n = run + int(a), wait + int(b), slices + int(c), n + 1
        except OSError:
            break
        f.write(json.dumps({"t": t, "io": {k: int(v) for k, v in io.items()},
                            "sched": [run, wait, slices, n]}) + "\n")
        time.sleep(0.1)
