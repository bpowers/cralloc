#!/usr/bin/env bash
# Builds the experiment's preload libraries and native workload builds.
# Run under scripts/heavy.
set -euo pipefail
S=/tmp/claude-1000/-home-bpowers-src-asmalloc/9350b66e-ff5f-4010-842a-f49ee9af0052/scratchpad
W=/home/bpowers/src/asmalloc/.claude/worktrees/agent-a25b3cca499ac1074

preload() { # tree name features
  local tree=$1 name=$2 feats=$3
  (cd "$S/src-$tree" && cargo build -q --release -p asmalloc-preload \
     ${feats:+--features "$feats"} --target-dir "$W/target/x-$tree")
  cp "$W/target/x-$tree/release/libasmalloc_preload.so" "$S/libs/$name.so"
  echo "built $name"
}

# A copy of the prototype's tree whose allocator crate has `feats` on by default, for native
# builds (bench-alloc does not pass the experiment's features through).
variant_tree() { # name feats
  local name=$1 feats=$2
  rm -rf "$S/src-$name"
  cp -a "$S/src-dom" "$S/src-$name"
  sed -i "s/^\[features\]$/[features]\ndefault = [$feats]/" "$S/src-$name/crates/asmalloc/Cargo.toml"
}

native() { # tree name features
  local tree=$1 name=$2 feats=$3
  (cd "$S/src-$tree/bench" && cargo build -q --release -p bench-workloads \
     --features "$feats" --target-dir "$W/bench/target/$name")
  echo "built native $name"
}

what="${1:-all}"
case "$what" in
  domrh)
    preload dom domrh "x-cap-relax,x-home"
    preload dom domrh-count "x-count,x-cap-relax,x-home"
    variant_tree domrh '"x-cap-relax", "x-home"'
    native domrh asm-domrh "bench-alloc/asmalloc"
    native domrh asm-domrh-dw "bench-alloc/asmalloc-domains"
    ;;
  preload|all)
    preload base base ""
    preload dom dom ""
    preload dom domof "x-own-fill-first"
    preload dom basof "x-no-affinity,x-own-fill-first"
    preload dom domc "x-carve-own-first"
    preload dom domcof "x-carve-own-first,x-own-fill-first"
    preload dom domp "x-peek"
    preload dom domcp "x-carve-own-first,x-peek"
    preload dom domph "x-home"
    preload dom domcph "x-carve-own-first,x-home"
    preload dom domrh "x-cap-relax,x-home"
    preload dom base-count "x-count,x-no-affinity"
    preload dom dom-count "x-count"
    preload dom domof-count "x-count,x-own-fill-first"
    preload dom basof-count "x-count,x-no-affinity,x-own-fill-first"
    preload dom domc-count "x-count,x-carve-own-first"
    preload dom domcof-count "x-count,x-carve-own-first,x-own-fill-first"
    preload dom domp-count "x-count,x-peek"
    preload dom domcp-count "x-count,x-carve-own-first,x-peek"
    preload dom domph-count "x-count,x-home"
    preload dom domcph-count "x-count,x-carve-own-first,x-home"
    preload dom domrh-count "x-count,x-cap-relax,x-home"
    ;;&
  native|all)
    variant_tree domof '"x-own-fill-first"'
    variant_tree basof '"x-no-affinity", "x-own-fill-first"'
    variant_tree domcof '"x-carve-own-first", "x-own-fill-first"'
    variant_tree domp '"x-peek"'
    variant_tree domcp '"x-carve-own-first", "x-peek"'
    variant_tree domph '"x-home"'
    variant_tree domcph '"x-carve-own-first", "x-home"'
    variant_tree domrh '"x-cap-relax", "x-home"'
    native base asm-base "bench-alloc/asmalloc"
    native dom asm-dom "bench-alloc/asmalloc"
    native domof asm-domof "bench-alloc/asmalloc"
    native basof asm-basof "bench-alloc/asmalloc"
    native domcof asm-domcof "bench-alloc/asmalloc"
    native domp asm-domp "bench-alloc/asmalloc"
    native domcp asm-domcp "bench-alloc/asmalloc"
    native domph asm-domph "bench-alloc/asmalloc"
    native domcph asm-domcph "bench-alloc/asmalloc"
    native domrh asm-domrh "bench-alloc/asmalloc"
    native base asm-base-dw "bench-alloc/asmalloc-domains"
    native dom asm-dom-dw "bench-alloc/asmalloc-domains"
    native domof asm-domof-dw "bench-alloc/asmalloc-domains"
    native domcp asm-domcp-dw "bench-alloc/asmalloc-domains"
    native domp asm-domp-dw "bench-alloc/asmalloc-domains"
    native domph asm-domph-dw "bench-alloc/asmalloc-domains"
    native domcph asm-domcph-dw "bench-alloc/asmalloc-domains"
    native domrh asm-domrh-dw "bench-alloc/asmalloc-domains"
    ;;
esac
