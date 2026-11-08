#!/usr/bin/env bash
# The rust-alloc loops under each variant build.sh made, rounds interleaved, each under scripts/measure,
# to check that a variant does what it says: `chainK` adds 2K a pair where the hop binds, `freeK` none.
#
#   loops.sh COPY VARIANT...      RF_ROUNDS (3) rounds, RF_FILTER a workload prefix
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
scripts=$here/../../../../scripts
copy=$1
shift
tmp=$(mktemp)
trap 'rm -f "$tmp"' EXIT
for _ in $(seq "${RF_ROUNDS:-3}"); do
  for v in "$@"; do
    MEMLIMIT_QUIET=1 "$scripts/measure" "$scripts/memlimit" -m 8G -t 10m -- env RF_SINGLE_ONLY=1 \
      "$copy/bench/roofline/rust-alloc/target/cralloc-$v/lto/roofline-rust-alloc" "$v" "${RF_FILTER:-}" >> "$tmp"
  done
done
python3 - "$tmp" "$@" <<'PY'
import collections
import sys
best = collections.defaultdict(lambda: collections.defaultdict(list))
ins = {}
for line in open(sys.argv[1]):
    f = line.split()
    if len(f) < 8 or f[3] != "cyc/op":
        continue
    best[f[1]][f[0]].append(float(f[2]))
    ins[f[1], f[0]] = float(f[4])
variants = sys.argv[2:]
print(f"{'workload':<14}" + "".join(f"{v:>16}" for v in variants))
for w in best:
    row = f"{w:<14}"
    for v in variants:
        c = best[w][v]
        row += f"{min(c):>9.2f} +{max(c) - min(c):<5.2f}" if c else " " * 16
    print(row)
print("instructions")
for w in best:
    print(f"{w:<14}" + "".join(f"{ins.get((w, v), 0):>16.1f}" for v in variants))
PY
