#!/usr/bin/env bash
# Build the slab stand-in (src/slab.rs) in each of its variants under fat LTO, then run the
# single-thread workloads on each, in interleaved rounds.
#
#   ./run_slab.sh [workload-prefix]
#
# RF_BUILD_CPUS (default 8-11) confines builds; RF_ROUNDS (default 3) is the number of rounds;
# RF_SLAB_VARIANTS overrides the variant list (cargo feature sets, `+` between features).
set -euo pipefail
cd "$(dirname "$0")"
variants=(${RF_SLAB_VARIANTS:-slab slab+slab_direct slab+slab_call slab+slab_call+slab_direct slab_ool slab_pc})
for v in "${variants[@]}"; do
  taskset -c "${RF_BUILD_CPUS:-8-11}" cargo build -q --profile lto --features "${v//+/,}" --target-dir "target/$v"
done
for _ in $(seq "${RF_ROUNDS:-3}"); do
  for v in "${variants[@]}"; do
    "target/$v/lto/roofline-rust-alloc" "$v/lto" "${1:-pair}"
  done
done
