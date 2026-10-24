#!/usr/bin/env bash
# Builds zbench against main's allocator with the prototype (proto_edit.py), under target/ of
# this checkout. Run under scripts/heavy:
#
#   scripts/heavy bench/probes/meili-peak/zbench.sh COMMIT
#
# then measure the two modes on one idle core, interleaved:
#
#   scripts/measure taskset -c 2 env ASMALLOC_ZPURGE=0000000 target/zbench/release/zbench
#   scripts/measure taskset -c 2 env ASMALLOC_ZPURGE=0000001 target/zbench/release/zbench
set -euo pipefail
commit="$1"
here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/../../.." && pwd)"
tree="$root/target/meili-peak-zbench"
rm -rf "$tree"
mkdir -p "$tree"
git -C "$root" archive "$commit" | tar -x -C "$tree"
mkdir -p "$tree/bench/probes/meili-peak"
cp -r "$here"/* "$tree/bench/probes/meili-peak/"
python3 -I "$tree/bench/probes/meili-peak/proto_edit.py" "$tree"
unset RUSTFLAGS CARGO_ENCODED_RUSTFLAGS
(cd "$tree/bench/probes/meili-peak/zbench" && cargo build --release --target-dir "$root/target/zbench")
rm -rf "$tree"
