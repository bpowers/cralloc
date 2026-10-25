#!/usr/bin/env bash
# Builds the allocator at COMMIT with the address-space trace (wl_edit.py) into the servers the
# note measures, as the bench build NAME of this checkout: the RPC servers (the log store and
# the HTTP/2 jobs) and the order service and Meilisearch apps. As grown-slots/build.sh: the
# commit's tree is extracted under target/ of this checkout (/tmp is memory), edited, built,
# and removed.
#
#   scripts/heavy bench/probes/first-touch/build.sh NAME COMMIT [PART...]
#
# PARTs (default: rpc orders meilisearch): rpc, into bench/target/NAME/release/rpc-server;
# an app of bench/apps, into bench/target/NAME/apps/PROGRAM/release/PROGRAM. With PLAIN=1 the
# edit is not applied.
set -euo pipefail
name="$1" commit="$2"
shift 2
parts=("$@")
[ ${#parts[@]} -gt 0 ] || parts=(rpc orders meilisearch)
here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/../../.." && pwd)"
shared="$(dirname "$(git -C "$root" rev-parse --path-format=absolute --git-common-dir)")/bench"
tree="$root/target/first-touch-$name"
out="$root/bench/target/$name"
rm -rf "$tree"
mkdir -p "$tree" "$out"
git -C "$root" archive "$commit" | tar -x -C "$tree"
git -C "$root" rev-parse "$commit" > "$out/BUILT-FROM"
if [ -z "${PLAIN:-}" ]; then
  python3 -I "$here/wl_edit.py" "$tree"
  echo "wl_edit.py $(sha256sum "$here/wl_edit.py" | cut -c1-12), wl.rs $(sha256sum "$here/wl.rs" | cut -c1-12)" >> "$out/BUILT-FROM"
fi
unset RUSTFLAGS CARGO_ENCODED_RUSTFLAGS BENCH_LAYOUT_PAD
export BENCH_BASELINES="$shared/baselines/out"
for part in "${parts[@]}"; do
  case "$part" in
    rpc)
      (cd "$tree/bench" && cargo build --release -p bench-rpc --features bench-alloc/asmalloc --target-dir "$out")
      ;;
    *)
      target="$out/apps/$part"
      mkdir -p "$target"
      (cd "$tree/bench/apps/$part" && CARGO_BUILD_BUILD_DIR="$shared/apps/target/build/$part" \
        cargo build --release --locked --features asmalloc --target-dir "$target")
      ;;
  esac
done
rm -rf "$tree"
echo "$name: $(tr '\n' ' ' < "$out/BUILT-FROM") [${parts[*]}]"
