#!/usr/bin/env bash
# The small-heap jobs of memory.md under the domain-split builds, at each domain width.
# usage: floor.sh <rounds> <width|l3> ...   (l3: the machine's own domains)
S=/tmp/claude-1000/-home-bpowers-src-asmalloc/9350b66e-ff5f-4010-842a-f49ee9af0052/scratchpad
W=/home/bpowers/src/asmalloc/.claude/worktrees/agent-a25b3cca499ac1074
rounds=$1; shift
for w in "$@"; do
  if [ "$w" = l3 ]; then unset ASMALLOC_DOMAIN_WIDTH; else export ASMALLOC_DOMAIN_WIDTH=$w; fi
  OUT="$S/floor/$w" bash "$W/bench/probes/memory/acct.sh" "$W" ${LABELS:-asm-base-dw,asm-dom-dw,asm-domcph-dw} "$rounds" \
    tiny churn1k pipe2 pipemix mpmc44 spawn4 | sed "s/^/width=$w /"
done
