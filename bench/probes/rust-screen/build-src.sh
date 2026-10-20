#!/usr/bin/env bash
# Builds the candidates fetched by sources.sh, with the system allocator, and records the
# build time in ROOT/builds.jsonl. Run under scripts/heavy.
#
# Usage: build-src.sh NAME...   (ruff, rust-analyzer, oxc, uv)
#
# Where a program picks an allocator of its own, this says how it is turned off:
#   ruff (ruff and ty): tikv-jemallocator is the #[global_allocator] on Linux x86-64
#     unconditionally; the attribute is removed, so jemalloc stays linked but unused.
#   oxc (oxlint): mimalloc behind the `allocator` feature, which release builds enable; not
#     enabled here, nor `napi`.
#   uv: jemalloc on Linux through the default `performance` feature; built without it.
#   egglog: mimalloc is the #[global_allocator] under the default `bin` feature; the
#     attribute is removed.
#   rust-analyzer: optional `mimalloc` and `jemalloc` features, off by default.
# The toolchain is the stable one (1.99.1) for all of them, not the 1.99.0 the repositories
# pin.
set -uo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
root="${RUST_SCREEN_ROOT:-$here/../../../target/rust-screen}"
root="$(cd "$root" && pwd)"
export RUSTUP_TOOLCHAIN=stable

record() { # NAME VERSION STATUS SECONDS ARGS
  printf '{"name":"%s","crate":"source","version":"%s","args":"%s","status":%d,"build_s":%d}\n' \
    "$1" "$2" "$5" "$3" "$4" >>"$root/builds.jsonl"
  echo "$1: status $3 in $4 s"
}

build() { # NAME VERSION DIR BINS... -- CARGO ARGS...
  local name="$1" version="$2" dir="$3"
  shift 3
  local bins=()
  while [ "$1" != "--" ]; do bins+=("$1"); shift; done
  shift
  local target="$root/build/$name"
  local start status=0
  start=$(date +%s)
  (cd "$dir" && CARGO_TARGET_DIR="$target" cargo build --release --locked "$@") >"$root/build/$name.log" 2>&1 || status=$?
  mkdir -p "$root/bin/$name"
  for b in "${bins[@]}"; do cp -f "$target/release/$b" "$root/bin/$name/" || status=1; done
  record "$name" "$version" "$status" $(( $(date +%s) - start )) "$*"
  tail -3 "$root/build/$name.log"
  rm -rf "$target"
}

for name in "$@"; do
  src="$root/src/$name"
  case "$name" in
    ruff)
      python3 - "$src" <<'EOF'
import sys, pathlib
for f in ["crates/ruff/src/main.rs", "crates/ty/src/main.rs"]:
    p = pathlib.Path(sys.argv[1]) / f
    s = p.read_text()
    old = "#[global_allocator]\nstatic GLOBAL: tikv_jemallocator::Jemalloc"
    if old in s:
        p.write_text(s.replace(old, "#[allow(dead_code)]\nstatic GLOBAL: tikv_jemallocator::Jemalloc"))
EOF
      build ruff 0.17.1 "$src" ruff ty -- -p ruff -p ty
      ;;
    rust-analyzer)
      build rust-analyzer 2026-10-12 "$src" rust-analyzer -- -p rust-analyzer
      ;;
    oxc)
      build oxc oxlint_v1.88.0 "$src" oxlint -- -p oxlint --no-default-features
      ;;
    uv)
      build uv 0.13.1 "$src" uv -- -p uv --no-default-features
      ;;
    egglog)
      # From the crates.io tarball (sources.sh): mimalloc is the #[global_allocator] of the
      # binary whenever its `bin` feature is on, so the attribute is removed.
      sed -i 's/^#\[global_allocator\]$/#[allow(dead_code)]/' "$src/src/main.rs"
      # It sits under this repository's workspace, so it needs one of its own.
      grep -q '^\[workspace\]' "$src/Cargo.toml" || printf '\n[workspace]\n' >>"$src/Cargo.toml"
      build egglog 3.0.0 "$src" egglog -- --bin egglog
      ;;
    datafusion | difftastic)
      # crates.io tarballs whose main.rs sets mimalloc (datafusion-cli) or jemalloc (difftastic)
      # as the #[global_allocator] unconditionally on Linux; the attribute is removed.
      sed -i 's/^#\[global_allocator\]$/#[allow(dead_code)]/' "$src/src/main.rs"
      grep -q '^\[workspace\]' "$src/Cargo.toml" || printf '\n[workspace]\n' >>"$src/Cargo.toml"
      case "$name" in
        datafusion) build datafusion 54.1.0 "$src" datafusion-cli -- --bin datafusion-cli ;;
        difftastic) build difftastic 0.71.0 "$src" difft -- --bin difft ;;
      esac
      ;;
    *) echo "unknown $name" >&2 ;;
  esac
done
