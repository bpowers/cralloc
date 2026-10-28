#!/usr/bin/env python3
"""Lists the ladder's runs that did not end cleanly or whose watcher saw a pause of more than
three seconds (the heavy slice frozen by a measurement), whose curves in time are then not
to be read: rerun them.

    python3 -I gaps.py OUTDIR [--remove]

With --remove, deletes those runs' files so that ladder.sh runs them again.
"""
import json
import sys
from pathlib import Path

for f in sorted(Path(sys.argv[1]).glob("*.txt")):
    w = Path(str(f) + ".watch.jsonl")
    lines = f.read_text().rstrip().splitlines()
    ok = bool(lines) and lines[-1].startswith("# exit 0")
    ts = [json.loads(l)["t"] for l in w.read_text().splitlines()[1:]] if w.exists() else []
    gap = max((b - a for a, b in zip(ts, ts[1:])), default=0)
    if not ok or gap > 3:
        print(f.name, "ok" if ok else "unfinished", f"gap {gap:.1f} s")
        if "--remove" in sys.argv:
            f.unlink()
            if w.exists():
                w.unlink()
