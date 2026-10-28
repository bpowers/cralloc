#!/usr/bin/env bash
# One soak run (soak.py) through scripts/heavy, its server under scripts/memlimit.
#
#   run.sh qdrant|connect asm|je OUT HOURS [SCALE] [REF]
#
# The binaries are build.sh's (bench/target/soak-*); the request inputs are under
# target/soak/inputs-KIND (qdrant_requests.py; docs/research/soak.md, "Method"). `asm`
# runs with the memstat sampler (one sample every 20 s) and the checks, against REF (the
# jemalloc run's OUT) when given; `je` is the reference and stops only if its server dies.
# SCALE divides every length, for a trial.
set -euo pipefail
kind="$1" alloc="$2" out="$3" hours="$4" scale="${5:-1}" ref="${6:-}"
here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../../.." && pwd)
shared_inputs=${SOAK_QDRANT_DATA:-$(dirname "$(git -C "$repo" rev-parse --path-format=absolute --git-common-dir)")/bench/apps/target/inputs/qdrant}
inputs=${SOAK_INPUTS:-$repo/target/soak/inputs-$kind}
mkdir -p "$out"
out=$(cd "$out" && pwd)
case "$kind/$alloc" in
  qdrant/asm) bin=(bench/target/soak-qdrant/apps/qdrant/release/qdrant serve "data=$shared_inputs") mem=5G ;;
  qdrant/je) bin=(bench/target/soak-qdrant-je/qdrant serve "data=$shared_inputs") mem=5G ;;
  connect/asm) bin=(bench/target/soak-rpc-asm/release/rpc-server store keep=1000000) mem=4G ;;
  connect/je) bin=(bench/target/soak-rpc-je/release/rpc-server store keep=1000000) mem=4G ;;
  *) echo "run.sh: qdrant|connect asm|je" >&2; exit 2 ;;
esac
flags=()
if [ "$alloc" = asm ]; then
  export ASMALLOC_MEMSTAT="$out/ms" ASMALLOC_MEMSTAT_MS=20000
  flags+=(--memstat "$out/ms")
  [ -n "$ref" ] && flags+=(--ref "$(cd "$ref" && pwd)")
else
  flags+=(--reference)
fi
cd "$repo"
exec scripts/heavy python3 -I "$here/soak.py" --kind "$kind" --out "$out" --hours "$hours" \
  --scale "$scale" --inputs "$inputs" "${flags[@]}" -- \
  scripts/memlimit -m "$mem" -t "$(python3 -c "print(int($hours * 60 / $scale) + 120)")m" -- "${bin[@]}"
