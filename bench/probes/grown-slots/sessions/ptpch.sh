#!/usr/bin/env bash
# tpchgen, the product build's preload library against main's, and glibc. From the checkout:
#   scripts/measure --both-l3 bench/probes/grown-slots/sessions/ptpch.sh OUT.jsonl
set -euo pipefail
t="$(pwd)/bench/target"
export RUST_SCREEN_ROOT="${RUST_SCREEN_ROOT:-/home/bpowers/src/asmalloc/.claude/worktrees/agent-ab8583755fe797b69/target/rust-screen}"
export APPF_OUT="$(pwd)/target/gs-appf"
mkdir -p "$APPF_OUT"
exec python3 -I bench/probes/grown-slots/tpch.py "$1" --rounds "${ROUNDS:-4}" \
  --arms "main=$t/ocm/libasmalloc_preload.so,rule=$t/oc/libasmalloc_preload.so,glibc=-"
