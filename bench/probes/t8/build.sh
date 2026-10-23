#!/usr/bin/env bash
# Builds the diagnostic workloads of t8_edit.py into bench/target/t8-NAME, from scratch copies
# in target/src-t8-NAME: `pre` (main before the fast lookup, 2754021), `post` (COMMIT, default
# HEAD) and `delay` (COMMIT with the delay). Run from the repository root under scripts/heavy.
# benchrun runs them through local, uncommitted allocators.toml entries:
#
#   [[allocator]]
#   name = "t8-post"
#   feature = "asmalloc"
#   build = "t8-post"
#   expect = { allocator = "asmalloc", library = "executable" }
set -euo pipefail
W=$(cd "$(dirname "$0")/../../.." && pwd)
commit="${1:-HEAD}"
common=$(git -C "$W" rev-parse --path-format=absolute --git-common-dir)/..
export BENCH_BASELINES="${BENCH_BASELINES:-$common/bench/baselines/out}"
unset RUSTFLAGS CARGO_ENCODED_RUSTFLAGS BENCH_LAYOUT_PAD
for spec in ${T8_BUILDS:-pre:2754021 post:$commit delay:$commit}; do
  name="${spec%%:*}" rev="${spec#*:}"
  src="$W/target/src-t8-$name"
  rm -rf "$src" && mkdir -p "$src"
  git -C "$W" archive --format=tar "$rev" | tar -x -C "$src"
  if [ "$name" = delay ]; then
    python3 "$W/bench/probes/t8/t8_edit.py" "$src" delay
  else
    python3 "$W/bench/probes/t8/t8_edit.py" "$src"
  fi
  (cd "$src/bench" && cargo build --release -q -p bench-workloads --features bench-alloc/asmalloc \
     --target-dir "$W/bench/target/t8-$name")
  echo "built $name"
done
