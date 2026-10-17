#!/usr/bin/env python3
"""Checks the module discipline of lean/Asmalloc/, which keeps the trusted
base small and separate from proofs. Exits non-zero on violations.

  * Every file under Asmalloc/ is a Lean module (it starts with `module`,
    after any leading comment). The audits run only in files that are not
    modules (the emitter's driver, AsmallocTest/).
  * TCB/   imports only Lean core and TCB/.
  * Spec/  imports only TCB/ and Spec/.
  * Impl/  imports only TCB/, Spec/ and Impl/: never proofs.
  * A module of a target (one with a directory of TCB/, e.g. `X86_64`, as a
    component of its path) imports no module of another target. What more
    than one target uses goes in a target-independent module.
"""

import pathlib
import re
import sys

LEAN = pathlib.Path(__file__).resolve().parent.parent
LIB = LEAN / "Asmalloc"

# The start of an import: `import`, and in a module `public`, `meta` and
# `all` imports.
IMPORT = r"\s*(?:public\s+)?(?:meta\s+)?import\s+(?:all\s+)?"

ALLOWED_IMPORTS = {
    "TCB": ("Lean", "Init", "Asmalloc.TCB."),
    "Spec": ("Asmalloc.TCB.", "Asmalloc.Spec."),
    "Impl": ("Asmalloc.TCB.", "Asmalloc.Spec.", "Asmalloc.Impl."),
}


def allowed(mod: str, prefixes: tuple[str, ...]) -> bool:
    return any(mod == p or mod.startswith(p if p.endswith(".") else p + ".") for p in prefixes)


def target_of(parts, targets: set[str]) -> str | None:
    return next((p for p in parts if p in targets), None)


def is_module(text: str) -> bool:
    """Whether `text` starts with `module`, after any leading comments."""
    comment = False
    for l in text.split("\n"):
        s = l.strip()
        if comment:
            comment = not s.endswith("-/")
            continue
        if s.startswith("/-") and not s.startswith("/-!"):
            comment = not s.endswith("-/")
            continue
        if s == "module":
            return True
        if s and not s.startswith("--"):
            return False
    return False


def main() -> int:
    errors = []
    targets = {p.name for p in (LIB / "TCB").iterdir() if p.is_dir()}
    for f in sorted(LIB.rglob("*.lean")):
        rel = f.relative_to(LEAN)
        if not is_module(f.read_text()):
            errors.append(f"{rel}: not a module; every file under Asmalloc/ starts with `module`")
        own = target_of(f.relative_to(LIB).with_suffix("").parts, targets)
        if own is None:
            continue
        for n, line in enumerate(f.read_text().splitlines(), 1):
            m = re.match(IMPORT + r"(Asmalloc\.\S+)", line)
            other = m and target_of(m.group(1).split("."), targets)
            if other and other != own:
                errors.append(f"{rel}:{n}: a module of {own} may not import {m.group(1)}, of {other}")
    for d, prefixes in ALLOWED_IMPORTS.items():
        for f in sorted((LIB / d).rglob("*.lean")):
            for n, line in enumerate(f.read_text().splitlines(), 1):
                m = re.match(IMPORT + r"(\S+)", line)
                if m and not allowed(m.group(1), prefixes):
                    errors.append(f"{f.relative_to(LEAN)}:{n}: {d}/ may not import {m.group(1)}")
    for e in errors:
        print(e, file=sys.stderr)
    if not errors:
        print("Lean imports OK")
    return 1 if errors else 0


if __name__ == "__main__":
    sys.exit(main())
