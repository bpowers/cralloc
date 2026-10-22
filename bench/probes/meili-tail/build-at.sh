#!/usr/bin/env bash
# Builds Meilisearch with the allocator at a commit, optionally edited by a script, as the
# bench build NAME beside this checkout's own (docs/research/meili-tail.md). The commit's tree
# is extracted under target/ of this checkout (gitignored; /tmp is memory), and its
# bench/apps/meilisearch is built as `benchrun build-apps` builds it for asmalloc, sharing the
# main checkout's build directory for the program's crates; the binary is copied to
# bench/target/NAME/apps/meilisearch/release/meilisearch of this checkout. Only Meilisearch:
# the workloads and the other programs are not needed for this note.
#
#   scripts/heavy bench/probes/meili-tail/build-at.sh NAME COMMIT [EDIT-SCRIPT]
#
# For example `asmalloc-parent 2754021`, or `asmalloc-switch 1eea9af
# bench/probes/meili-tail/switch_edit.py`. Then a local entry in bench/allocators.toml (not
# committed), as session.sh describes.
set -euo pipefail
name="$1" commit="$2" edit="${3:-}"
here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/../../.." && pwd)"
shared="$(dirname "$(git -C "$root" rev-parse --path-format=absolute --git-common-dir)")/bench"
tree="$root/target/meili-tail-$name"
rm -rf "$tree"
mkdir -p "$tree"
git -C "$root" archive "$commit" | tar -x -C "$tree"
echo "$(git -C "$root" rev-parse "$commit") ${edit:+edited by $(basename "$edit") $(sha256sum "$edit" | cut -c1-12)}" > "$tree/BUILT-FROM"
if [ -n "$edit" ]; then
  python3 -I "$edit" "$tree"
fi
target="$root/bench/target/$name/apps/meilisearch"
mkdir -p "$target"
cd "$tree/bench/apps/meilisearch"
env -u RUSTFLAGS -u CARGO_ENCODED_RUSTFLAGS -u BENCH_LAYOUT_PAD \
  CARGO_BUILD_BUILD_DIR="$shared/apps/target/build/meilisearch" BENCH_BASELINES="$shared/baselines/out" \
  cargo build --release --locked --features asmalloc --target-dir "$target"
cp "$tree/BUILT-FROM" "$root/bench/target/$name/BUILT-FROM"
echo "$name: $(cat "$tree/BUILT-FROM")"
