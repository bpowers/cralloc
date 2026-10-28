#!/usr/bin/env bash
# The soak's builds, from a clean tree of COMMIT: Qdrant with asmalloc and the memstat sampler
# (bench/target/soak-qdrant, a long fat-LTO link), and rpc-server with asmalloc and the sampler
# and with jemalloc (bench/target/soak-rpc-asm, soak-rpc-je). Run it through scripts/heavy.
# The jemalloc Qdrant is the suite's `je` build of bench/apps/qdrant, copied to
# bench/target/soak-qdrant-je/qdrant.
#
#   build.sh COMMIT qdrant|rpc
#
# The sampler writes to ASMALLOC_MEMSTAT.<pid> (ms_pid.py), since Qdrant's driver and the
# server it starts are the same binary.
set -euo pipefail
commit="$1" what="$2"
here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../../.." && pwd)
shared=$(dirname "$(git -C "$repo" rev-parse --path-format=absolute --git-common-dir)")/bench
tree="$repo/target/soak/tree-$what"
rm -rf "$tree"; mkdir -p "$tree"
git -C "$repo" archive "$commit" | tar -x -C "$tree"
python3 -I "$here/ms_pid.py" "$tree/bench/alloc/src/memstat.rs"
unset RUSTFLAGS CARGO_ENCODED_RUSTFLAGS BENCH_LAYOUT_PAD
export BENCH_BASELINES="$shared/baselines/out"
case "$what" in
  qdrant)
    sed -i 's|^je = \["bench-alloc/je"\]$|je = ["bench-alloc/je"]\nasmalloc-memstat = ["bench-alloc/asmalloc-memstat"]|' \
      "$tree/bench/apps/qdrant/Cargo.toml"
    grep -q asmalloc-memstat "$tree/bench/apps/qdrant/Cargo.toml"
    export PATH="$shared/apps/target/inputs/qdrant/bin:$PATH"
    (cd "$tree/bench/apps/qdrant" && CARGO_BUILD_BUILD_DIR="$shared/apps/target/build/qdrant" \
      cargo build --release --features asmalloc-memstat --target-dir "$repo/bench/target/soak-qdrant/apps/qdrant")
    ;;
  rpc)
    for spec in asm:bench-alloc/asmalloc-memstat je:bench-alloc/je; do
      (cd "$tree/bench" && cargo build --release -p bench-rpc --bin rpc-server --features "${spec#*:}" \
        --target-dir "$repo/bench/target/soak-rpc-${spec%%:*}")
    done
    ;;
  *) echo "build.sh COMMIT qdrant|rpc" >&2; exit 2 ;;
esac
rm -rf "$tree"
echo "$what built from $commit"
