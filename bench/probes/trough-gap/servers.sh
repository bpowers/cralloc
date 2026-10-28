#!/usr/bin/env bash
# One timed session of the order service at 2 GiB (servers-large, `orders axum cache 2G`) with
# the variants of docs/research/trough-gap.md, benchrun interleaving them in a random order. Run
# from the repository root with allocators.local.toml appended to bench/allocators.toml:
#
#   scripts/measure --both-l3 bench/probes/trough-gap/servers.sh OUTDIR ROUND [ALLOCATORS]
#
# ALLOCATORS defaults to the study's variants; the refresh rule's sessions name
# tg-rx-off,tg-rx,live-bytes: one binary, the switch by environment.
set -euo pipefail
out="$(realpath -m "$1")" round="$2" allocators="${3:-tg-base,tg-r4,tg-m256,live-bytes}"
here="$(cd "$(dirname "$0")" && pwd)"
bench="$(cd "$here/../.." && pwd)"
mkdir -p "$out"
cd "$bench"
./target/release/benchrun run --suite servers-large --only "=orders axum cache 2G" \
  --allocators "$allocators" --rounds 1 --mem-cap 7G \
  --out "$out/orders-2G-r$round.jsonl"
