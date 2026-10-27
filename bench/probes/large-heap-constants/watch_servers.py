#!/usr/bin/env python3
"""Watches the servers a benchrun session starts, for docs/research/large-heap-constants.md:
every PERIOD seconds, for each live process whose executable is one of EXE, a sample
(watchlib.py: resident, page tables, mappings, faults, the background thread, and the diagnostic
build's counters). One JSON object a line, {"pid", "exe", "start", ...sample}. Runs until SIGTERM.

    watch_servers.py OUT PERIOD EXE...
"""
import json
import os
import signal
import sys
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.dont_write_bytecode = True
sys.path.insert(0, str(HERE))
import watchlib  # noqa: E402

out, period = Path(sys.argv[1]), float(sys.argv[2])
exes = {str(Path(e).resolve()) for e in sys.argv[3:]}
stop = False


def on_term(*_):
    global stop
    stop = True


signal.signal(signal.SIGTERM, on_term)
signal.signal(signal.SIGINT, on_term)
watches = {}
with open(out, "a") as f:
    while not stop:
        now = time.monotonic()
        for d in Path("/proc").iterdir():
            if not d.name.isdigit():
                continue
            pid = int(d.name)
            if pid in watches:
                continue
            try:
                exe = os.path.realpath(f"/proc/{pid}/exe")
            except OSError:
                continue
            if exe in exes:
                watches[pid] = (exe, watchlib.Watch(pid, exe, now), time.time())
        for pid, (exe, w, start) in list(watches.items()):
            rec = w.sample(now)
            if rec is None or rec.get("rss_anon") is None:
                del watches[pid]
                continue
            rec.update(pid=pid, exe=exe, start=start)
            f.write(json.dumps(rec, separators=(",", ":")) + "\n")
        f.flush()
        time.sleep(period)
