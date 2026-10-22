#!/usr/bin/env bash
# meili.sh OUT ALLOCATORS ROUNDS: the Meilisearch job (`meilisearch hn`, 930 req/s) through
# benchrun, under the measure lock; then moves the series files its creditdiag builds wrote to
# OUT.series/.
set -uo pipefail
W=/home/bpowers/src/asmalloc/.claude/worktrees/agent-ac0a2195f8109ecfc
O="$W/bench/target/hc-runs/meili"
mkdir -p "$O/$1.series"
before=$(date +%s.%N)
cd "$W/bench"
"$W/scripts/measure" --both-l3 ./target/release/benchrun run --suite servers-large --only "=meilisearch hn" \
  --allocators "$2" --rounds "$3" --mem-cap 10G --gate-wait 120 --out "$O/$1.jsonl" > "$O/$1.log" 2>&1
echo "exit $?"
find "$W/bench/target/hc-series" -name '*.series' -newermt "@$before" -exec mv {} "$O/$1.series/" \;
