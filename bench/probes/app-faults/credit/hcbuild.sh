#!/usr/bin/env bash
# hcbuild.sh TAG PROGRAM FEATURE [EDIT.py...]: copies this worktree's crates and bench to
# bench/target/hc-src/TAG, runs the edit scripts in the copy, and builds bench/apps/PROGRAM there
# with FEATURE into bench/target/TAG/apps/PROGRAM (where benchrun's `build = TAG` finds it).
# Run under scripts/heavy.
set -euo pipefail
W=/home/bpowers/src/asmalloc/.claude/worktrees/agent-ac0a2195f8109ecfc
tag="$1"; program="$2"; feature="$3"; shift 3
edits=()
for p in "$@"; do edits+=("$(realpath "$p")"); done
D="$W/bench/target/hc-src/$tag"
rm -rf "$D"; mkdir -p "$D/bench"
cp -r "$W/Cargo.toml" "$W/Cargo.lock" "$W/crates" "$D/"
(cd "$W/bench" && tar --exclude=./target --exclude=./apps/target -cf - .) | (cd "$D/bench" && tar -xf -)
cd "$D"
for p in ${edits[@]+"${edits[@]}"}; do python3 -I "$p"; done
cd "$D/bench/apps/$program"
env -u RUSTFLAGS -u CARGO_ENCODED_RUSTFLAGS \
  CARGO_BUILD_BUILD_DIR=/home/bpowers/src/asmalloc/bench/apps/target/build/$program \
  BENCH_BASELINES=/home/bpowers/src/asmalloc/bench/baselines/out \
  cargo build --release --locked --target-dir "$W/bench/target/$tag/apps/$program" --features "$feature" 2>&1 \
  | grep -E "^(error|warning)|Finished" -A6 || true
ls -la "$W/bench/target/$tag/apps/$program/release/$program"
