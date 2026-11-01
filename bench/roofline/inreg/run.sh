#!/usr/bin/env bash
# Is the last cycle of the Rust fast path the fixed input registers of the inline routines?
# Builds bench/roofline/rust-alloc's `asm` and `asm_exe` variants (fat LTO) twice: with the
# generated inline pop and push as they are, and with their inputs as `in(reg)` operands
# (patch.py). Then runs the single-thread workloads on RF_CPU (default 12) in RF_ROUNDS
# interleaved rounds (default 3).
#
# The patched builds come from a copy of rust-alloc and the allocator crate under target/, laid
# out as in the repository so that rust-alloc's path dependency finds the copy. The checked-in
# generated file is never edited, so an interrupted run leaves nothing behind.
#
#   run.sh [workload-prefix]
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
repo=$(git -C "$here" rev-parse --show-toplevel)
tgt=$here/target
cpus=${RF_BUILD_CPUS:-4-7,20-23}
tree=$tgt/tree
rm -rf "$tree"
mkdir -p "$tree/bench/roofline/rust-alloc" "$tree/crates"
cp "$repo/Cargo.toml" "$tree/"
cp -r "$repo/crates/cralloc" "$tree/crates/"
cp -r "$repo/bench/roofline/rust-alloc/Cargo.toml" "$repo/bench/roofline/rust-alloc/build.rs" \
  "$repo/bench/roofline/rust-alloc/src" "$tree/bench/roofline/rust-alloc/"
gen=$tree/crates/cralloc/src/front/generated/x86_64_inline/slab.rs
cp "$gen" "$tgt/slab.rs.orig"
build() { # tag features
  (cd "$tree/bench/roofline/rust-alloc" &&
    taskset -c "$cpus" cargo build -q --profile lto --features "$2" --target-dir "$tgt/$1")
}
build fixed asm
build fixed_exe asm_exe
python3 "$here/patch.py" "$gen"
build inreg asm
build inreg_exe asm_exe
cp "$tgt/slab.rs.orig" "$gen"
python3 "$here/patch.py" --onereg "$gen"
build onereg asm
build onereg_exe asm_exe
for _ in $(seq "${RF_ROUNDS:-3}"); do
  for v in fixed inreg onereg fixed_exe inreg_exe onereg_exe; do
    RF_CPU=${RF_CPU:-12} MEMLIMIT_QUIET=1 "$repo/scripts/memlimit" -m 2G -t 10m -- \
      "$tgt/$v/lto/roofline-rust-alloc" "$v" "${1:-pair}"
  done
done
