#!/usr/bin/env bash
# servers.sh with the servers' huge pages sampled beside it (thpwatch.sh, into OUT.thp) and
# the free 2 MiB blocks recorded before. From the checkout:
#   scripts/measure --both-l3 bench/probes/huge-tail/sessions/servers-watch.sh OUT.jsonl SUITE [ONLY]
set -euo pipefail
out="$1"
echo "$(date +%s%3N) free2m=$(bench/probes/huge-tail/free2m.sh)" >> "$out.thp"
bench/probes/huge-tail/thpwatch.sh "${WATCH_BUILD:-ht6}" "$out.thp" &
watch=$!
trap 'kill "$watch" 2>/dev/null' EXIT
bench/probes/huge-tail/sessions/servers.sh "$@"
