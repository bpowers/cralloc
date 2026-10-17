#!/usr/bin/env bash
# Every check of lean/: the import discipline, the registration files, every
# proof and golden test (warnings are errors), that the generated Rust is what
# Lean emits, and that the axiom audit still rejects a hidden axiom.
# Needs `lake` on PATH (elan installs it in ~/.elan/bin). Pass the output
# directory if it is not `generated/`.
set -euo pipefail
cd "$(dirname "$0")/.."

python3 scripts/check_lean_imports.py
python3 scripts/check_registrations.py
lake build --log-level=warning
# Replays every module through the kernel: a declaration that skipped it
# (`debug.skipKernelTC`) is rejected here.
lake env leanchecker Asmalloc
lake env lean --run Emit.lean --check "$@"
python3 scripts/audit_negative.py
# The negative test leaves the self-test built from the files it edited.
lake build --log-level=warning
