#!/usr/bin/env python3
"""spin.py CPU SECONDS: spin on CPU under SCHED_IDLE for SECONDS, so that the CPU runs this
instead of entering an idle state whenever nothing else wants it, and any other task that wakes
on it displaces it at once. A way to take idle states (and their exit latency) out of a
server's wake-ups without privileges: `echo 1 > .../cpuidle/stateN/disable` needs root.
"""
import os
import sys
import time

cpu, secs = int(sys.argv[1]), float(sys.argv[2])
os.sched_setaffinity(0, {cpu})
os.sched_setscheduler(0, os.SCHED_IDLE, os.sched_param(0))
end = time.monotonic() + secs
while time.monotonic() < end:
    for _ in range(100_000):
        pass
