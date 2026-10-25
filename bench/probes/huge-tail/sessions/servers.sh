#!/usr/bin/env bash
# One round of a large-heap server job with the switch off and on (one binary), in a random
# order, as the standings run them (12 GiB cap). From the checkout:
#   scripts/measure --both-l3 bench/probes/huge-tail/sessions/servers.sh OUT.jsonl SUITE [ONLY]
# SUITE is servers-thread with ONLY one of "=orders axum cache 2G thread", "=connect store 1M
# thread", "=meilisearch hn thread", or meili-tail. Needs the local allocators.toml entries
# ht-off and ht-on (build = "ht2", ASMALLOC_HT = "t0" and "t1"; not committed).
set -euo pipefail
out="$1" suite="$2" only="${3:-}"
cd bench
args=(run --suite "$suite" --allocators "${ALLOCS:-ht-off,ht-on}" --rounds "${ROUNDS:-1}" --mem-cap 12G --out "../$out")
[ -n "$only" ] && args+=(--only "$only")
exec target/release/benchrun "${args[@]}"
