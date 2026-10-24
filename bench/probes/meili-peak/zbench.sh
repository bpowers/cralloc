#!/usr/bin/env bash
# Builds zbench against the allocator at COMMIT with the edits given (the switch of the rule
# being measured), into target/zbench-TAG of this checkout. Run under scripts/heavy:
#
#   scripts/heavy bench/probes/meili-peak/zbench.sh TAG COMMIT [EDIT...]
#
# The note's builds: `fixed 169b5c7 proto_edit.py` and `evidence <the rule's commit>
# evidence_edit.py`. Then the modes, one at a time on one idle core:
#
#   scripts/measure scripts/memlimit -m 2G -- taskset -c 2 env ASMALLOC_UNWRITTEN=0 \
#       target/zbench-evidence/release/zbench
set -euo pipefail
tag="$1" commit="$2"
shift 2
here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/../../.." && pwd)"
tree="$root/target/meili-peak-zbench-$tag"
rm -rf "$tree"
mkdir -p "$tree"
git -C "$root" archive "$commit" | tar -x -C "$tree"
mkdir -p "$tree/bench/probes/meili-peak"
cp -r "$here"/* "$tree/bench/probes/meili-peak/"
for edit in "$@"; do
  python3 -I "$tree/bench/probes/meili-peak/$(basename "$edit")" "$tree"
done
unset RUSTFLAGS CARGO_ENCODED_RUSTFLAGS
(cd "$tree/bench/probes/meili-peak/zbench" && cargo build --release --target-dir "$root/target/zbench-$tag")
rm -rf "$tree"
