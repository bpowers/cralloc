#!/usr/bin/env bash
# Every check of lean/: the import discipline, the registration files, every
# proof and golden test (warnings are errors), that the generated Rust is what
# Lean emits, the size and alignment of its naked functions, that the AArch64
# printer prints what the model means, and that the
# axiom audit still rejects a hidden axiom. When all pass, it records the
# generated files' checksums for the merge gate (scripts/generated-check).
# Needs `lake` on PATH (elan installs it in ~/.elan/bin). Pass the output
# directory if it is not the allocator crate's `src/front/generated/`.
set -euo pipefail
# The kernel checker at full thread count goes over 16 GiB on the shared machine, above any
# cap we may run under; four threads keep the whole check under 4 GiB.
export LEAN_NUM_THREADS="${LEAN_NUM_THREADS:-4}"
cd "$(dirname "$0")/.."

python3 scripts/check_lean_imports.py
python3 scripts/check_registrations.py
lake build --log-level=warning
# Replays every module under `Cralloc` through the kernel: a declaration that skipped it
# (`debug.skipKernelTC`) is rejected here. The emitter's audit refuses to load any other, but
# Lean's own (`VG.foreignModules`), so this is every module it loads.
lake env leanchecker Cralloc
lake env lean --run Emit.lean --check "$@"
# Each naked function's bytes and alignment, assembled (needs llvm-mc).
python3 scripts/check_sizes.py "$@"
# The AArch64 printer's encodings, and the model against a processor where one is at hand.
python3 scripts/check_aarch64.py
python3 scripts/audit_negative.py
# The negative test leaves the self-test built from the files it edited.
lake build --log-level=warning
# Everything passed, so the checked-in output is what Lean emits: record its checksums, which
# the merge gate compares in milliseconds (scripts/generated-check). Only for the default
# output directory.
if [ $# -eq 0 ]; then
  ../scripts/generated-check --write
fi
