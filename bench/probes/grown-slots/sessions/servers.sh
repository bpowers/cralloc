#!/usr/bin/env bash
# One round of a large-heap server job under the given allocators, in a random order, as the
# standings ran them (12 GiB cap). From the checkout:
#   scripts/measure --both-l3 bench/probes/grown-slots/sessions/servers.sh OUT.jsonl ALLOCATORS SUITE [ONLY]
# SUITE is meili-tail (Meilisearch: the fill, then two minutes at 70%) or servers-thread with
# ONLY one of "=orders axum cache 2G thread", "=connect store 1M thread", "=meilisearch hn thread".
set -euo pipefail
out="$1" allocs="$2" suite="$3" only="${4:-}"
cd bench
args=(run --suite "$suite" --allocators "$allocs" --rounds 1 --mem-cap 12G --out "../$out")
[ -n "$only" ] && args+=(--only "$only")
exec target/release/benchrun "${args[@]}"
