#!/usr/bin/env bash
# One timed session of the order service at 2 GiB (servers-large, `orders axum cache 2G`) with
# the variants of docs/research/trough-gap.md, benchrun interleaving them in a random order. Run
# from the repository root with allocators.local.toml appended to bench/allocators.toml:
#
#   scripts/measure --both-l3 bench/probes/trough-gap/servers.sh OUTDIR ROUND
set -euo pipefail
out="$(realpath -m "$1")" round="$2"
here="$(cd "$(dirname "$0")" && pwd)"
bench="$(cd "$here/../.." && pwd)"
mkdir -p "$out"
cd "$bench"
./target/release/benchrun run --suite servers-large --only "=orders axum cache 2G" \
  --allocators "tg-base,tg-r4,tg-m256,live-bytes" --rounds 1 --mem-cap 7G \
  --out "$out/orders-2G-r$round.jsonl"
