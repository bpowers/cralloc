#!/usr/bin/env bash
# Copies the incumbents' builds of the bench programs and apps, and the load generator, from
# another checkout's bench/target into this one's, for the sessions of
# docs/research/whole-block-huge.md. They do not depend on asmalloc, so the standings' builds
# serve; copied rather than linked, so that the other checkout may go.
#
#   bench/probes/whole-block-huge/copy-incumbents.sh FROM_BENCH_TARGET
set -euo pipefail
from="$1"
to="$(cd "$(dirname "$0")/../.." && pwd)/target"
for a in je mi tc; do
  mkdir -p "$to/$a/release"
  cp "$from/$a/release/workload" "$from/$a/release/rpc-server" "$to/$a/release/"
  for p in datafusion meilisearch orders; do
    mkdir -p "$to/$a/apps/$p/release"
    cp "$from/$a/apps/$p/release/$p" "$to/$a/apps/$p/release/"
  done
done
mkdir -p "$to/loadgen/release"
cp "$from/loadgen/release/loadgen" "$from/loadgen/release/upstream" "$to/loadgen/release/"
echo "copied from $from" > "$to/INCUMBENTS-FROM"
