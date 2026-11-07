#!/usr/bin/env bash
# Builds an inlined fast path each way that matters and says which link and run. Cases:
#   stub in .subsection 1, stub in .text.unlikely, stub through a `label` operand, no stub;
#   a small binary, a function past 1 MiB, 300 MiB of text before the rest;
#   __rseq_offset through the GOT, or directly by the system linker and by rust-lld, as a PIE and
#   not.
set -u
cd "$(dirname "$0")"
sysroot=$(rustc --print sysroot)
host=$(rustc -vV | sed -n 's/^host: //p')
lld_dir="$sysroot/lib/rustlib/$host/bin/gcc-ld"
export CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-target}

try() {
  local name=$1 features=$2 flags=$3
  local log; log=$(mktemp)
  if RUSTFLAGS="$flags" cargo build --release --features "$features" >"$log" 2>&1; then
    if out=$("$CARGO_TARGET_DIR/release/roofline-reach" 2>&1); then
      echo "ok    $name: $out"
    else
      echo "RUN   $name: $out"
    fi
  else
    echo "FAIL  $name: $(grep -E 'error|relocation|out of range|truncated' "$log" | grep -v '^error: could not' | head -2 | sed -e 's/.*\.o: *//' | cut -c1-150 | tr '\n' ' ')"
  fi
  rm -f "$log"
}

try "subsection, small" "" ""
try "subsection, function past 1 MiB" "big_function" ""
try "subsection, 300 MiB of text" "big_text" ""
try "unlikely, small" "stub_unlikely" ""
try "unlikely, function past 1 MiB" "stub_unlikely,big_function" ""
try "unlikely, 300 MiB of text" "stub_unlikely,big_text" ""
try "label, small" "stub_label" ""
try "label, function past 1 MiB" "stub_label,big_function" ""
try "label, 300 MiB of text" "stub_label,big_text" ""
try "no stub, small" "stub_none" ""
try "no stub, function past 1 MiB" "stub_none,big_function" ""
try "no stub, 300 MiB of text" "stub_none,big_text" ""
try "direct offset, system linker, PIE" "direct_offset" ""
try "direct offset, system linker, no PIE" "direct_offset" "-C link-arg=-no-pie -C relocation-model=static"
try "direct offset, rust-lld, PIE" "direct_offset" "-C link-arg=-B$lld_dir -C link-arg=-fuse-ld=lld"
try "direct offset, rust-lld, no PIE" "direct_offset" "-C link-arg=-B$lld_dir -C link-arg=-fuse-ld=lld -C link-arg=-no-pie -C relocation-model=static"
try "GOT offset, rust-lld, PIE" "" "-C link-arg=-B$lld_dir -C link-arg=-fuse-ld=lld"

# Where a stub in a subsection lands: after the function's return, in the same section.
RUSTFLAGS="" cargo build --release >/dev/null 2>&1
llvm-objdump -d --no-show-raw-insn "$CARGO_TARGET_DIR/release/roofline-reach" 2>/dev/null \
  | awk '/roofline_reach.*hot.*>:$/{p=1} p&&/^$/{exit} p' | cut -c1-70
