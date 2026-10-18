#!/usr/bin/env bash
# Is the last cycle of the Rust fast path the fixed input registers of the inline routines?
# Builds bench/roofline/rust-alloc's `asm` and `asm_exe` variants (fat LTO) twice: with the
# generated inline pop and push as they are, and with their inputs as `in(reg)` operands
# (patch.py, applied to a copy of the generated file and undone afterwards). Then runs the
# single-thread workloads on RF_CPU (default 12) in RF_ROUNDS interleaved rounds (default 3).
#
#   run.sh [workload-prefix]
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
repo=$(git -C "$here" rev-parse --show-toplevel)
gen=$repo/crates/asmalloc/src/front/generated/x86_64_inline/slab.rs
bench=$repo/bench/roofline/rust-alloc
tgt=$here/target
cpus=${RF_BUILD_CPUS:-4-7,20-23}
build() { # tag features
  (cd "$bench" && taskset -c "$cpus" cargo build -q --profile lto --features "$2" --target-dir "$tgt/$1")
}
build fixed asm
build fixed_exe asm_exe
cp "$gen" "$tgt/slab.rs.orig"
trap 'cp "$tgt/slab.rs.orig" "$gen"' EXIT
python3 "$here/patch.py" "$gen"
build inreg asm
build inreg_exe asm_exe
cp "$tgt/slab.rs.orig" "$gen"
trap - EXIT
for _ in $(seq "${RF_ROUNDS:-3}"); do
  for v in fixed inreg fixed_exe inreg_exe; do
    RF_CPU=${RF_CPU:-12} MEMLIMIT_QUIET=1 "$repo/scripts/memlimit" -m 2G -t 10m -- \
      "$tgt/$v/lto/roofline-rust-alloc" "$v" "${1:-pair}"
  done
done
