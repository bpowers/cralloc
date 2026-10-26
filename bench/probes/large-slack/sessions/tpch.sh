#!/usr/bin/env bash
# tpchgen under the switch's arms and glibc. From the checkout:
#   scripts/measure --both-l3 bench/probes/large-slack/sessions/tpch.sh OUT.jsonl [ROUNDS]
# RUST_SCREEN_ROOT names the screening's tree (bench/probes/rust-screen); the default is the
# one the grown-slots study used.
set -euo pipefail
lib="$(pwd)/bench/target/lsr/libasmalloc_preload.so"
ref="$(pwd)/bench/target/lsr-main/libasmalloc_preload.so"
export RUST_SCREEN_ROOT="${RUST_SCREEN_ROOT:-/home/bpowers/src/asmalloc/.claude/worktrees/agent-ab8583755fe797b69/target/rust-screen}"
export APPF_OUT="$(pwd)/target/large-slack/appf"
mkdir -p "$APPF_OUT"
exec python3 -I bench/probes/large-slack/tpch.py "$1" --rounds "${2:-5}" \
  --arms "off=$lib:t0a0,trim=$lib:t1a0,both=$lib:t1a1,main=$ref:t1a1"
