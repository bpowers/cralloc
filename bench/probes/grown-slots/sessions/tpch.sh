#!/usr/bin/env bash
# tpchgen under the switch's settings and glibc. From the checkout:
#   scripts/measure --both-l3 bench/probes/grown-slots/sessions/tpch.sh OUT.jsonl ARMS [--diag]
# ARMS as tpch.py takes them, with LIB written as @ for this checkout's gs preload library.
set -euo pipefail
lib="$(pwd)/bench/target/gs/libasmalloc_preload.so"
export RUST_SCREEN_ROOT="${RUST_SCREEN_ROOT:-/home/bpowers/src/asmalloc/.claude/worktrees/agent-ab8583755fe797b69/target/rust-screen}"
export APPF_OUT="$(pwd)/target/gs-appf"
mkdir -p "$APPF_OUT"
out="$1" arms="${2//@/$lib}"
shift 2
exec python3 -I bench/probes/grown-slots/tpch.py "$out" --rounds "${ROUNDS:-5}" --arms "$arms" "$@"
