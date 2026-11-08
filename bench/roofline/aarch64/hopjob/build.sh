#!/usr/bin/env bash
# Builds the suite's workloads and the rust-alloc loops, under each variant of the routines
# (variant.py), in a scratch copy of the repository, so that the checkout stays as it is:
#
#   build.sh COPY VARIANT...       `benchrun run` in COPY/bench then runs them as allocators
#
# Each variant becomes an allocator `cralloc-VARIANT` in COPY's bench/allocators.toml. A variant may
# carry a pad, VARIANT@N, which moves all the code after `bench-alloc`'s pad by N bytes (`benchrun
# layout`'s pads), for comparing variants over placements.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../../../.." && pwd)
copy=$1
shift
mkdir -p "$copy"
rsync -a --delete --exclude target --exclude .git --exclude '*.orig' --exclude out --exclude results \
  --exclude 'bench/allocators.toml' --exclude third_party "$root/" "$copy/" 2>/dev/null || true
[ -f "$copy/bench/allocators.toml" ] || cp "$root/bench/allocators.toml" "$copy/bench/allocators.toml"
# variant.py keeps the routines as they came beside them; they just came again.
find "$copy/crates/cralloc/src" -name '*.orig' -delete
common=$(cd "$root" && git rev-parse --path-format=absolute --git-common-dir)
export BENCH_BASELINES=${BENCH_BASELINES:-$(dirname "$common")/bench/baselines/out}
page=$(getconf PAGESIZE)
export CFLAGS_aarch64_unknown_linux_gnu=-mcpu=native
export CXXFLAGS_aarch64_unknown_linux_gnu="-mcpu=native -DSNMALLOC_PAGESIZE=$page"
for spec in "$@"; do
  variant=${spec%@*}
  name=cralloc-$variant
  pad=
  if [ "$spec" != "$variant" ]; then
    pad=${spec#*@}
    name=$name-pad$pad
  fi
  python3 "$here/variant.py" "$copy" "$variant"
  grep -q "^name = \"$name\"$" "$copy/bench/allocators.toml" || cat >> "$copy/bench/allocators.toml" <<EOF

[[allocator]]
name = "$name"
feature = "cralloc"
build = "$name"
expect = { allocator = "cralloc", library = "executable" }
EOF
  envs=()
  [ "$variant" = kept ] && envs+=(RUSTC_BOOTSTRAP=1)
  [ -n "$pad" ] && envs+=(BENCH_LAYOUT_PAD="$pad")
  # Linked by rust-lld, the one linker that copies __rseq_offset into a PIE: the routines as they are
  # and with that direct read, so that the two differ by nothing else.
  sysroot=$(rustc --print sysroot)
  lld="-Ctarget-cpu=native -C link-arg=-fuse-ld=lld -C link-arg=-B$sysroot/lib/rustlib/aarch64-unknown-linux-gnu/bin/gcc-ld"
  [ "$variant" = lld ] && envs+=(RUSTFLAGS="$lld")
  [ "$variant" = direct ] && envs+=(RUSTFLAGS="$lld --cfg cralloc_a64_direct")
  (
    cd "$copy/bench"
    "$root/scripts/heavy" env ${envs[@]+"${envs[@]}"} cargo build -q --release -p bench-workloads -p bench-rpc \
      --features bench-alloc/cralloc --target-dir "target/$name"
    [ -z "$pad" ] || exit 0
    cd roofline/rust-alloc
    "$root/scripts/heavy" env ${envs[@]+"${envs[@]}"} cargo build -q --profile lto --features asm --target-dir "target/$name"
  )
  echo "built $name"
done
