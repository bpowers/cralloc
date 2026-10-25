#!/usr/bin/env python3
"""The switch's counters (`d1`, the last line of each FILE.ht), with cycles turned into
microseconds at the reference host's TSC (4.3 GHz).

    python3 -I bench/probes/huge-tail/htsumm.py FILE.ht...
"""
import re
import sys

TSC_MHZ = 4300.0


def main():
    print("| file | down | up | splits | MiB returned | split mean/max us | raise mean/max us | lag mean/max us | walk call in a tail mean/max us | thread or slow-path s | return-to-lowering window mean/max ms |")
    print("|---|---|---|---|---|---|---|---|---|---|---|")
    for f in sys.argv[1:]:
        lines = [l for l in open(f) if l.startswith("ht ")]
        if not lines:
            continue
        kv = {k: int(v) for k, v in re.findall(r"(\w+)=(\d+)", lines[-1])}
        us = lambda c: c / TSC_MHZ
        mean = lambda a, n: us(kv.get(a, 0)) / max(1, kv.get(n, 0))
        calls = sum(kv.get(k, 0) for k in ("unadvise_cyc", "readvise_cyc", "split_cyc", "tail_purge_cyc"))
        print(
            f"| {f.split('/')[-2]}/{f.split('/')[-1]} | {kv['unadvise']} | {kv['readvise']} | {kv['splits']} | {kv['tail_pages'] * 4 / 1024:.0f} | "
            f"{mean('split_cyc', 'splits'):.0f} / {us(kv['split_max']):.0f} | {mean('readvise_cyc', 'readvise'):.0f} / {us(kv['readvise_max']):.0f} | "
            f"{mean('lag_cyc', 'readvise'):.0f} / {us(kv['lag_max']):.0f} | "
            f"{mean('step_cyc', 'tail_purges') if 'step_cyc' in kv else float('nan'):.0f} / {us(kv.get('step_max', 0)):.0f} | {us(calls) / 1e6:.2f} | {mean('window_cyc', 'windows') / 1e3:.1f} / {us(kv.get('window_max', 0)) / 1e3:.1f} |"
        )


if __name__ == "__main__":
    main()
