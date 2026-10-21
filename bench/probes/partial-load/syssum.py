#!/usr/bin/env python3
"""Memory system calls per 1,000 requests from run.py syscalls runs (strace -c output):

  syssum.py DIR...
"""
import json
import re
import sys
from pathlib import Path

CALLS = ["madvise", "mprotect", "mmap", "munmap", "mremap", "brk"]
# run.py's server names, which may hold dashes, to find where a tag's build begins.
SERVERS = re.findall(r'^    "([^"]+)": \(', (Path(__file__).parent / "run.py").read_text(), re.M)


def build_of(tag):
    server = max((s for s in SERVERS if tag.startswith(s + "-")), key=len, default=tag.split("-", 1)[0])
    return tag[len(server) + 1:].rsplit("-", 1)[0]

for d in sys.argv[1:]:
    print(f"\n{Path(d).name}: calls per 1,000 requests, and the total seconds strace saw in them")
    print("  " + f"{'build':15s}" + "".join(f"{c:>10s}" for c in CALLS) + "   seconds")
    for p in sorted(Path(d).glob("*.syscalls")):
        tag = p.name[: -len(".syscalls")]
        n = json.loads((p.parent / f"{tag}.result.json").read_text())["groups"]["m"]["lat_us"]["n"]
        calls, secs = {}, 0.0
        for line in p.read_text().splitlines():
            f = line.split()
            if f and f[-1] in CALLS:
                # % time, seconds, usecs/call, calls, [errors,] syscall
                calls[f[-1]] = int(f[3])
                secs += float(f[1])
        print(f"  {build_of(tag):15s}"
              + "".join(f"{calls.get(c, 0) * 1000 / n:10.3f}" for c in CALLS) + f"   {secs:7.3f}")
