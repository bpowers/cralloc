#!/usr/bin/env bash
# Builds the counting build (count_edit.py) of the three servers into bench/target/asmalloc-prof,
# from a scratch copy of COMMIT (default HEAD) in target/src-prof. Run from the repository root
# under scripts/heavy. benchrun runs it through a local, uncommitted allocators.toml entry:
#
#   [[allocator]]
#   name = "asmalloc-prof"
#   feature = "asmalloc"
#   build = "asmalloc-prof"
#   env = { BENCH_PROF_DIR = "<absolute directory for the traces>" }
#   expect = { allocator = "asmalloc", library = "executable" }
set -euo pipefail
W=$(cd "$(dirname "$0")/../../.." && pwd)
commit="${1:-HEAD}"
common=$(git -C "$W" rev-parse --path-format=absolute --git-common-dir)/..
export BENCH_BASELINES="${BENCH_BASELINES:-$common/bench/baselines/out}"
data="${BENCH_APPS_DATA:-$common/bench/apps/target}"
unset RUSTFLAGS CARGO_ENCODED_RUSTFLAGS
src="$W/target/src-prof"
rm -rf "$src" && mkdir -p "$src"
git -C "$W" archive --format=tar "$commit" | tar -x -C "$src"
python3 "$W/bench/probes/server-profile/count_edit.py" "$src"
out="$W/bench/target/asmalloc-prof"
(cd "$src/bench" && cargo build --release -p bench-rpc --features bench-alloc/prof --target-dir "$out")
for app in orders meilisearch; do
  (cd "$src/bench/apps/$app" && CARGO_BUILD_BUILD_DIR="$data/build/$app" \
     cargo build --release --locked --features prof --target-dir "$out/apps/$app")
done
ls -la "$out/release/rpc-server" "$out/apps/orders/release/orders" "$out/apps/meilisearch/release/meilisearch"
