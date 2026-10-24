#!/usr/bin/env python3
"""Request files for the mini-redis run of docs/research/heap-drift.md.

    python3 -I bench/probes/heap-drift/requests.py OUTDIR [KEYS]

Writes OUTDIR/<name>-<KEYS>.resp for each size band below: every key of a server preloaded
with KEYS keys (`miniredis serve preload=KEYS`, keys `key:00000000` on) SET once, in a seeded
random order, to a value of a size drawn log-uniformly from the band, each SET followed by a
GET of a random key. Sent once from end to end, a file overwrites every key at the band's
sizes, the old values going in random order; the load generator sends a file once when its
segment lasts as long as the file's commands take at the segment's rate.
"""
import random
import sys
from pathlib import Path

BANDS = {"wide": (64, 4096), "small": (32, 256), "mid": (256, 2048)}


def cmd(*parts):
    out = [b"*%d\r\n" % len(parts)]
    for p in parts:
        out.append(b"$%d\r\n%s\r\n" % (len(p), p))
    return b"".join(out)


def main():
    dest = Path(sys.argv[1])
    keys = int(sys.argv[2]) if len(sys.argv) > 2 else 1_200_000
    dest.mkdir(parents=True, exist_ok=True)
    for seed, (name, (lo, hi)) in enumerate(BANDS.items()):
        rng = random.Random(1000 + seed)
        order = list(range(keys))
        rng.shuffle(order)
        path = dest / f"{name}-{keys}.resp"
        with open(path, "wb", buffering=1 << 22) as f:
            for i, k in enumerate(order):
                n = int(lo * (hi / lo) ** rng.random())
                value = (b"%08d" % i) * (n // 8) + b"x" * (n % 8)
                f.write(cmd(b"SET", b"key:%08d" % k, value))
                f.write(cmd(b"GET", b"key:%08d" % rng.randrange(keys)))
        print(f"{path}: {path.stat().st_size / 2**20:.0f} MiB, {2 * keys} commands")


if __name__ == "__main__":
    main()
