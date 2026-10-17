#!/usr/bin/env bash
# What the toolchain does with two things the routines might rely on.
#
#   1. A direct rip-relative load of glibc's `__rseq_offset` (src/main.rs): the relocation the
#      linker produces in an executable, with rustc's default linker and with GNU ld, whether the
#      value is right from the first user code on, and that a shared library cannot link it
#      (shared.rs).
#   2. A `call` inside an `asm!` block that is not `nostack` (asmcall.rs): the code rustc
#      generates around it, for the stack-alignment and red-zone guarantees.
#
# RF_BUILD_CPUS (default 8-11) confines builds.
set -euo pipefail
cd "$(dirname "$0")"
cpus=${RF_BUILD_CPUS:-8-11}
out=target/check
mkdir -p "$out"

echo "== linker rustc uses by default"
taskset -c "$cpus" cargo rustc -q --release -- --print link-args | tr ' ' '\n' | grep -E '^"-fuse-ld|^"-pie|^"-Wl,-z' || true
for ld in default bfd; do
  flags=""
  [ "$ld" = bfd ] && flags="-C link-arg=-fuse-ld=bfd"
  echo "== executable, linker $ld"
  RUSTFLAGS="$flags" taskset -c "$cpus" cargo build -q --release --target-dir "$out/$ld"
  readelf -rW "$out/$ld/release/roofline-toolchain" | grep __rseq_offset
  readelf -SW "$out/$ld/release/roofline-toolchain" | grep -E '\.bss\.rel\.ro|\.data\.rel\.ro' || true
  "$out/$ld/release/roofline-toolchain"
  echo "== shared library, linker $ld (expected to fail)"
  if taskset -c "$cpus" rustc --edition 2021 -O --crate-type cdylib $flags -o "$out/libshared-$ld.so" shared.rs 2>"$out/shared-$ld.err"; then
    echo "linked (unexpected)"
  else
    grep -E 'relocation|recompile' "$out/shared-$ld.err" | head -2
  fi
done

echo "== asm! blocks that call (rustc -O --emit asm)"
taskset -c "$cpus" rustc --edition 2021 --crate-type cdylib -O -C panic=abort --emit asm -o "$out/asmcall.s" asmcall.rs
grep -vE '^\s*\.(cfi|p2align|prefalign|section|type|size|globl|file|ident|loc)|^\s*$|#(NO_)?APP' "$out/asmcall.s" \
  | awk '/^[a-z_]+:$/ {f=1} /^\.Lfunc_end/ {f=0} f'
