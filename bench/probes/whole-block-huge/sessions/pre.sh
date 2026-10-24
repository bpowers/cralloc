#!/usr/bin/env bash
# A program of the screening's tree under the switch's arms (this checkout's wbh preload
# library) and mimalloc's and tcmalloc's libraries. From the checkout:
#   scripts/measure --both-l3 bench/probes/whole-block-huge/sessions/pre.sh OUT.jsonl PROG
set -euo pipefail
lib="$(pwd)/bench/target/wbh/libasmalloc_preload.so"
base="$(dirname "$(git rev-parse --path-format=absolute --git-common-dir)")/bench/baselines/out"
export RUST_SCREEN_ROOT="${RUST_SCREEN_ROOT:-/home/bpowers/src/asmalloc/.claude/worktrees/agent-ab8583755fe797b69/target/rust-screen}"
export WBH_WORK="$(pwd)/target/wbh-work"
mkdir -p "$WBH_WORK"
exec python3 -I bench/probes/whole-block-huge/pre.py "$1" --prog "$2" --rounds "${ROUNDS:-3}" \
  --arms "main=$lib:m0f002d0,all2=$lib:m1f002d0,nz2=$lib:m2f002d0,all16=$lib:m1f016d0,mi=$base/mimalloc/libmimalloc.so:,tc=$base/tcmalloc/libtcmalloc.so:" "${@:3}"
