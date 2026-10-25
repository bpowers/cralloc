#!/usr/bin/env python3
"""The long calls of a session.sh trace, and purge durations by size.

  wllong.py OUTDIR TAG [--min-us 500]
"""
import argparse
import importlib.util
from pathlib import Path

spec = importlib.util.spec_from_file_location("wlsum", Path(__file__).with_name("wlsum.py"))
w = importlib.util.module_from_spec(spec)
spec.loader.exec_module(w)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("dir")
    ap.add_argument("tag")
    ap.add_argument("--min-us", type=float, default=500)
    a = ap.parse_args()
    for f in sorted((Path(a.dir) / f"{a.tag}.wl").glob("*.wl")):
        tf = f.with_suffix(".threads")
        names = {}
        if tf.exists():
            for line in tf.read_text().splitlines():
                t, _, n = line.partition(" ")
                names[int(t)] = n
        c = w.load_wl(f)
        if not c:
            continue
        t00 = c[0][0]
        long = [x for x in c if x[1] >= a.min_us * 1e3]
        print(f"== {f.stem}: {len(c)} calls, {len(long)} of {a.min_us:.0f} us or more")
        for t0, dur, tid, ln, op, ctx, addr, cpu, nv, niv in long:
            print(f"  {(t0 - t00) / 1e9:8.3f}s {dur / 1e3:8.0f} us {w.OPS[op]:10} {w.CTX[ctx]:7} "
                  f"{ln >> 10:9} KiB cpu {cpu / 1e3:6.0f} us, slept {nv}, preempted {niv}  {names.get(tid, tid)}")
        # How the long calls spent their time: on the CPU, asleep (a lock), or preempted.
        for name, sel in (("long", long), ("all", c)):
            if sel:
                on = sum(x[7] for x in sel) / sum(x[1] for x in sel)
                print(f"  {name} calls: {len(sel)}, CPU {on:.0%} of their time; slept in {sum(x[8] > 0 for x in sel)}, "
                      f"preempted in {sum(x[9] > 0 for x in sel)}")
        print("  purges by size: KiB range, calls, p50 / p99 / max us")
        purges = [x for x in c if x[4] == 2]
        for lo, hi in [(0, 64 << 10), (64 << 10, 1 << 20), (1 << 20, 8 << 20), (8 << 20, 1 << 40)]:
            d = sorted(x[1] for x in purges if lo <= x[3] < hi)
            if d:
                print(f"  {lo >> 10:>8}-{hi >> 10:<10} {len(d):7} {d[len(d) // 2] / 1e3:8.1f} "
                      f"{d[int(len(d) * .99)] / 1e3:8.1f} {d[-1] / 1e3:8.1f}")


if __name__ == "__main__":
    main()
