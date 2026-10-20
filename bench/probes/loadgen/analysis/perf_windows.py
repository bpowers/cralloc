#!/usr/bin/env python3
"""What the server's threads were doing in chosen windows of a run, against the whole run.

  perf_windows.py RUN_DIR head HEAD_MS [TOP]       the first HEAD_MS of each burst after silence
  perf_windows.py RUN_DIR slow SEGMENT LAT_US [TOP] while requests of SEGMENT slower than LAT_US waited

RUN_DIR holds perf.data (perf record -k CLOCK_MONOTONIC), raw.bin and summary.json. Prints the
samples per window-millisecond, the share of samples in the allocator (symbols in asmalloc::,
or the preloaded allocator's library), in the kernel, and the top symbols inside against
outside the windows."""
import bisect, collections, json, re, struct, subprocess, sys
from pathlib import Path

d = Path(sys.argv[1]); mode = sys.argv[2]
summ = json.load(open(d / "summary.json"))
t0 = summ["start_mono_ns"]
segs = summ["segments"]
if mode == "head":
    head = float(sys.argv[3]) * 1e6
    top = int(sys.argv[4]) if len(sys.argv) > 4 else 25
    wins = [[s["begin_ms"] * 1e6, s["begin_ms"] * 1e6 + head] for i, s in enumerate(segs)
            if s["name"] == "on" and segs[i - 1]["name"] == "off"]
    ref = [[s["begin_ms"] * 1e6 + 100e6, s["end_ms"] * 1e6] for i, s in enumerate(segs)
           if s["name"] == "on" and segs[i - 1]["name"] == "off"]
else:
    seg_name, lat_us = sys.argv[3], float(sys.argv[4])
    top = int(sys.argv[5]) if len(sys.argv) > 5 else 25
    idx = {i for i, s in enumerate(segs) if s["name"] == seg_name}
    w = sorted((due, due + lat) for due, lat, seg, fl in struct.iter_unpack("<QQHH", (d / "raw.bin").read_bytes())
               if seg in idx and lat > lat_us * 1e3)
    wins = []
    for a, b in w:
        if wins and a <= wins[-1][1]:
            wins[-1][1] = max(wins[-1][1], b)
        else:
            wins.append([a, b])
    ref = [[segs[i]["begin_ms"] * 1e6, segs[i]["end_ms"] * 1e6] for i in idx]


def index(ws):
    ws = sorted(ws)
    return [a for a, b in ws], ws


def inside(t, ix):
    starts, ws = ix
    k = bisect.bisect_right(starts, t) - 1
    return k >= 0 and t < ws[k][1]


wi, ri = index(wins), index(ref)
line_re = re.compile(r"^\s*(\d+)\s+([\d.]+):\s+[0-9a-f]+\s+(.*?)\s+\((.*)\)\s*$")
out = subprocess.run(["perf", "script", "-i", str(d / "perf.data"), "-F", "tid,time,ip,sym,dso", "--ns"],
                     capture_output=True, text=True).stdout
cin, cref = collections.Counter(), collections.Counter()
for line in out.splitlines():
    m = line_re.match(line)
    if not m:
        continue
    t = int(float(m.group(2)) * 1e9) - t0
    sym, dso = m.group(3).split("+0x")[0], m.group(4).split("/")[-1]
    if "asmalloc" in sym or "asmalloc" in dso or "jemalloc" in dso or "mimalloc" in dso or "tcmalloc" in dso:
        cls = "allocator"
    elif "kernel" in dso:
        cls = "kernel"
    else:
        cls = "program"
    key = (cls, sym[:100])
    if inside(t, wi):
        cin[key] += 1
    elif inside(t, ri):
        cref[key] += 1
span_in = sum(b - a for a, b in wins) / 1e6
span_ref = sum(b - a for a, b in ref) / 1e6
nin, nref = sum(cin.values()), sum(cref.values())
print(f"windows: {len(wins)}, {span_in:.1f} ms, {nin} samples ({nin / max(span_in, 1e-9):.0f}/ms); "
      f"reference: {span_ref:.0f} ms, {nref} samples ({nref / max(span_ref, 1e-9):.0f}/ms)")
for cls in ("allocator", "kernel", "program"):
    a = sum(v for k, v in cin.items() if k[0] == cls)
    b = sum(v for k, v in cref.items() if k[0] == cls)
    print(f"  {cls:9s} inside {a / max(nin, 1) * 100:5.1f}% ({a / max(span_in, 1e-9):6.1f}/ms)   "
          f"reference {b / max(nref, 1) * 100:5.1f}% ({b / max(span_ref, 1e-9):6.1f}/ms)")
print(f"{'inside %':>9s} {'ref %':>7s}  symbol")
for k, v in cin.most_common(top):
    print(f"{v / max(nin, 1) * 100:9.2f} {cref[k] / max(nref, 1) * 100:7.2f}  {k[0][:5]} {k[1]}")
