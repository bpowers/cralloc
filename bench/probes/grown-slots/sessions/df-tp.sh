#!/usr/bin/env bash
# DataFusion (six rounds) and tpchgen (four) under the ranked rules, one session. From the
# checkout: scripts/measure --both-l3 bench/probes/grown-slots/sessions/df-tp.sh DF.jsonl TP.jsonl
set -euo pipefail
ROUNDS=6 bench/probes/grown-slots/sessions/apps.sh "$1" gs-main,gs-fown,gs-fown64,gs-own64
ROUNDS=4 bench/probes/grown-slots/sessions/tpch.sh "$2" \
  'main=@:m0b0016s00r0d0,fown=@:m4b0016s00r0d0,fown64=@:m4b0064s00r0d0,own64=@:m1b0064s00r0d0,res=@:m0b0016s00r1d0,glibc=-'
