#!/usr/bin/env bash
# Builds programs of the apps and servers suites under each variant of the routines, where the
# runner in COPY finds them, from a second copy of the repository so that build.sh can go on in the
# first. The dependencies are compiled once, in a build directory beside that copy:
#
#   apps.sh COPY 'PROGRAM...' VARIANT...
#
# build.sh must have made cralloc-VARIANT in COPY's allocators.toml.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../../../.." && pwd)
copy=$1
programs=$2
shift 2
src=$copy-apps
mkdir -p "$src"
rsync -a --delete --exclude target --exclude .git --exclude '*.orig' --exclude out --exclude results \
  --exclude third_party "$root/" "$src/"
find "$src/crates/cralloc/src" -name '*.orig' -delete
common=$(cd "$root" && git rev-parse --path-format=absolute --git-common-dir)
export BENCH_BASELINES=${BENCH_BASELINES:-$(dirname "$common")/bench/baselines/out}
page=$(getconf PAGESIZE)
export CFLAGS_aarch64_unknown_linux_gnu=-mcpu=native
export CXXFLAGS_aarch64_unknown_linux_gnu="-mcpu=native -DSNMALLOC_PAGESIZE=$page"
inputs=${BENCH_APPS_DATA:-$(dirname "$common")/bench/apps/target}/inputs
for variant in "$@"; do
  python3 "$here/variant.py" "$src" "$variant"
  envs=()
  [ "$variant" = kept ] && envs+=(RUSTC_BOOTSTRAP=1)
  for program in $programs; do
    (
      [ -d "$inputs/$program/bin" ] && export PATH="$inputs/$program/bin:$PATH"
      cd "$src/bench/apps/$program"
      "$root/scripts/heavy" env ${envs[@]+"${envs[@]}"} CARGO_BUILD_BUILD_DIR="$src-build/$program" \
        cargo build -q --release --locked --features cralloc \
        --target-dir "$copy/bench/target/cralloc-$variant/apps/$program"
    )
    echo "built $program under $variant"
  done
done
