#!/usr/bin/env bash
# Build one binary per allocator and profile, then run them all.
#
#   MI_LIB_DIR=/path/to/dir/with/libmimalloc.a ./run.sh [workload-prefix]
#
# RF_BUILD_CPUS (default 4-7) is the CPU list builds are confined to, so they stay off the
# measurement CPUs. RF_CPU, RF_CPU2 and RF_PAIRS are read by the binaries.
set -euo pipefail
cd "$(dirname "$0")"
allocators=(sys mi mi_sized je floor_tls floor_heap asm asm_exe)
profiles=(release lto)
for a in "${allocators[@]}"; do
  features=()
  [ "$a" = sys ] || features=(--features "$a")
  if [ "$a" = mi ] || [ "$a" = mi_sized ]; then
    [ -n "${MI_LIB_DIR:-}" ] || { echo "skipping $a: MI_LIB_DIR not set" >&2; continue; }
  fi
  for p in "${profiles[@]}"; do
    taskset -c "${RF_BUILD_CPUS:-4-7}" cargo build -q --profile "$p" "${features[@]}" --target-dir "target/$a"
  done
done
for a in "${allocators[@]}"; do
  for p in "${profiles[@]}"; do
    bin="target/$a/$p/roofline-rust-alloc"
    [ -x "$bin" ] && "$bin" "$a/$p" "${1:-}"
  done
done
