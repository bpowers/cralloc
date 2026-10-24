#!/usr/bin/env bash
# Starts a build of Meilisearch with 100k documents, with its sampler on, and stops it after
# SECS seconds (the cap's timeout sends SIGTERM): a check that a sampling build runs and writes its file, not a measurement.
# Run under scripts/heavy:
#
#   scripts/heavy bench/probes/meili-peak/smoke.sh BUILD OUTDIR [SECS]
set -euo pipefail
build="$1" out="$2" secs="${3:-40}"
here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/../../.." && pwd)"
mkdir -p "$out"
export ASMALLOC_MEMSTAT="$out/ms" JEMALLOC_MEMSTAT="$out/js" ASMALLOC_MEMSTAT_MS=500 JEMALLOC_MEMSTAT_MS=500
"$root/scripts/memlimit" -m 8G -t "${secs}s" -- \
  "$root/bench/target/$build/apps/meilisearch/release/meilisearch" serve \
  data=/home/bpowers/src/asmalloc/bench/apps/target/inputs/meilisearch docs=1 > "$out/out.txt" 2>&1 || true
tail -3 "$out/out.txt"
ls -la "$out"
