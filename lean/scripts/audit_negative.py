#!/usr/bin/env python3
"""Checks that the axiom audit rejects an axiom hidden in a Lean module, and
that the audits refuse to run in one.

A module sees the theorems it imports from other modules as axioms, without
their proofs, so the audits run in the emitter's driver, a file that is not
a module, which sees every proof (`TCB/Axioms.lean`). This test proves that
they still see through modules, and through a private import, which hides a
module from the modules that import the importer, but not from a file that is
not a module. It adds `axiom vg_bad : False` and a theorem proven from it to a
module that an artifact depends on (`Spec/Selftest.lean`), and adds a module
with another such axiom, `Proof/Selftest/AuditPrivate.lean`, which the
self-test's proof imports with a plain `import`. It proves `add_correct` from
both theorems, and checks that `Emit.lean --check` fails with the audit's
`depends on non-standard axioms: [...]`, listing both
`VG.Spec.Selftest.vg_bad` and `VG.Proof.Selftest.AuditPrivate.vg_bad`: the
audit lists every axiom it finds, so one build checks both ways of hiding one.

It edits the files in place, after copying them to a temporary directory,
and restores them from there (and deletes the module it added) whatever
happens, then checks with `git diff` that the tree is as it was: the axiom
never reaches the tree that is committed.

It also runs every audit command in a scratch module, outside the tree,
and checks that each refuses to run there (Lean reports each command's error
and goes on to the next).

And it replaces the self-test's proof by each of two forgeries a review
found (`scripts/witnesses/`), and checks that the check rejects each:

* `skip_kernel`: a theorem of `False` elaborated with
  `debug.skipKernelTC`, which the kernel never sees, so no axiom appears;
  replaying the module through the kernel (`leanchecker`) must reject it.
* `audit_macro_hijack`: an axiom, and a `macro_rules` that rewrites
  `#assert_standard_axioms` into nothing in any file importing it; the
  emitter audits in its own process (`Emit.lean`), so it must still report
  the axiom.

Run it from anywhere after `lake build`: it rebuilds the few modules of the
self-test that the edits change, and the next `lake build` rebuilds them from
the restored files.

  scripts/audit_negative.py
"""

import pathlib
import re
import shutil
import subprocess
import sys
import tempfile

LEAN = pathlib.Path(__file__).resolve().parent.parent
# A module (`Spec/` is converted), and a file whose proof the artifact uses.
MODULE = LEAN / "Cralloc" / "Spec" / "Selftest.lean"
PROOF = LEAN / "Cralloc" / "Proof" / "Selftest" / "X86_64.lean"
AXIOM = "VG.Spec.Selftest.vg_bad"

INJECTED = """
/-- Not in the tree: `scripts/audit_negative.py` adds it to check the audit. -/
axiom vg_bad : False

theorem vg_bad_thm : False := vg_bad
"""

# The self-test's proof: its first theorem, proven from the axiom instead.
CORRECT = "theorem add_correct "

# The second case: a module with the axiom, imported privately.
PRIVATE = LEAN / "Cralloc" / "Proof" / "Selftest" / "AuditPrivate.lean"
PRIVATE_MODULE = "Cralloc.Proof.Selftest.AuditPrivate"
PRIVATE_AXIOM = "VG.Proof.Selftest.AuditPrivate.vg_bad"
PRIVATE_TEXT = """module

/-! Not in the tree: `scripts/audit_negative.py` adds it to check the audit. -/

public section

namespace VG.Proof.Selftest.AuditPrivate

axiom vg_bad : False

theorem vg_bad_thm : False := vg_bad

end VG.Proof.Selftest.AuditPrivate
"""


def is_module(text: str) -> bool:
    return any(l.strip() == "module" for l in text.split("\n")[:5])


def inject_module(text: str) -> str:
    """`text` with the axiom and its theorem at the end of its namespace."""
    end = "end VG.Spec.Selftest"
    if end not in text:
        raise SystemExit(f"{MODULE}: no `{end}` to add the axiom before")
    return text.replace(end, INJECTED.lstrip("\n") + "\n" + end, 1)


def inject_proof(text: str, proof: str) -> str:
    """`text` with `add_correct`'s proof replaced by one from `proof`, a
    proof of `False`."""
    start = text.find(CORRECT)
    if start < 0:
        raise SystemExit(f"{PROOF}: no `{CORRECT.strip()}`")
    body = text.find(":= by", start)
    end = text.find("\n\n", body)
    if body < 0 or end < 0:
        raise SystemExit(f"{PROOF}: `add_correct` is not `:= by …` followed by a blank line")
    return text[:body] + f":=\n  {proof}.elim" + text[end:]


IMPORT_LINE = re.compile(r"^(public\s+)?(meta\s+)?import\s")


def inject_private_import(text: str) -> str:
    """`text` (a module) with a plain `import` of the module with the axiom,
    after its last import, and `add_correct` proven from both axioms'
    theorems (each must reach the audit)."""
    lines = text.split("\n")
    last = max(i for i, l in enumerate(lines) if IMPORT_LINE.match(l))
    lines.insert(last + 1, f"import {PRIVATE_MODULE}")
    both = "⟨Spec.Selftest.vg_bad_thm, VG.Proof.Selftest.AuditPrivate.vg_bad_thm⟩"
    return inject_proof("\n".join(lines), f"({both} : False ∧ False).2")


# Each audit command, and an argument to give it in the scratch module.
AUDITS = [("#assert_standard_axioms", "VG.standardAxioms"),
          ("#assert_no_compiler_overrides", "VG.standardAxioms"),
          ("#assert_spec_origin", "")]


def audits_refused(tmp: str) -> bool:
    """Whether each audit command fails in a scratch module (in `tmp`),
    saying why."""
    f = pathlib.Path(tmp) / "Audit.lean"
    f.write_text("module\n\nimport Cralloc.TCB.Audit\nimport Cralloc.TCB.Axioms\n\n"
                 + "".join(f"{cmd} {arg}\n".replace(" \n", "\n") for cmd, arg in AUDITS))
    r = subprocess.run(["lake", "env", "lean", str(f)], cwd=LEAN, text=True,
                       stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
    ok = True
    for cmd, _ in AUDITS:
        if r.returncode == 0 or f"{cmd} must run in a file that is not a module" not in r.stdout:
            print(f"{cmd} ran in a module", file=sys.stderr)
            ok = False
    if not ok:
        print(r.stdout, file=sys.stderr)
    return ok


def emit_check(edits: dict, added: pathlib.Path) -> str | None:
    """Writes `edits` (file: new text) and `added` (with `PRIVATE_TEXT`), runs
    `Emit.lean --check`, restores the files and deletes `added` (and its
    build), and returns the output if it failed, else `None`."""
    with tempfile.TemporaryDirectory() as tmp:
        saved = {p: pathlib.Path(tmp) / p.name for p in edits}
        for p, copy in saved.items():
            shutil.copy2(p, copy)
        try:
            added.write_text(PRIVATE_TEXT)
            for p, text in edits.items():
                p.write_text(text)
            r = subprocess.run(["lake", "env", "lean", "--run", "Emit.lean", "--check"],
                               cwd=LEAN, capture_output=True, text=True)
        finally:
            for p, copy in saved.items():
                shutil.copy2(copy, p)
            added.unlink(missing_ok=True)
            build = LEAN / ".lake" / "build"
            for d in ("lib/lean", "ir"):
                for f in (build / d).glob(f"{added.relative_to(LEAN).with_suffix('')}.*"):
                    f.unlink()
    return None if r.returncode == 0 else r.stdout + r.stderr


# The audit's error: one line per axiom it found (`Emit.lean`, `audit`).
REJECTED = re.compile(r"^non-standard axiom: (\S+)$", re.M)


def rejected(out: str | None) -> bool:
    """Whether the audit rejected both axioms (`out`, from `emit_check`)."""
    axioms = set(REJECTED.findall(out or ""))
    ok = True
    for axiom, how in ((AXIOM, "in a module"), (PRIVATE_AXIOM, "behind a private import")):
        if axiom in axioms:
            print(f"the audit rejects an axiom {how}: `{axiom}`")
        else:
            print(f"Emit.lean --check did not reject `{axiom}`: "
                  f"the axiom audit no longer sees an axiom {how}", file=sys.stderr)
            ok = False
    if not ok:
        print(out or "", file=sys.stderr)
    return ok


WITNESSES = LEAN / "scripts" / "witnesses"


def forged(witness: str, cmd: list[str], expect: str) -> bool:
    """Replaces the self-test's proof by `witness`, builds it, runs `cmd`,
    restores the proof, and whether `cmd` failed saying `expect`."""
    with tempfile.TemporaryDirectory() as tmp:
        copy = pathlib.Path(tmp) / PROOF.name
        shutil.copy2(PROOF, copy)
        try:
            PROOF.write_text((WITNESSES / f"{witness}.lean.txt").read_text())
            b = subprocess.run(["lake", "build", "--log-level=warning",
                                "+Cralloc.Artifacts.Selftest.X86_64"],
                               cwd=LEAN, capture_output=True, text=True)
            if b.returncode != 0:
                print(f"the forgery {witness} does not build:\n{b.stdout}{b.stderr}", file=sys.stderr)
                return False
            r = subprocess.run(cmd, cwd=LEAN, capture_output=True, text=True)
        finally:
            shutil.copy2(copy, PROOF)
    out = r.stdout + r.stderr
    if r.returncode != 0 and expect in out:
        print(f"the check rejects the forgery {witness}")
        return True
    print(f"the check does not reject the forgery {witness}:\n{out}", file=sys.stderr)
    return False


def main() -> int:
    for f in (MODULE, PROOF):
        if not is_module(f.read_text()):
            raise SystemExit(f"{f} is not a module: the test needs one")
    if PRIVATE.exists():
        raise SystemExit(f"{PRIVATE} exists: the test adds it")
    with tempfile.TemporaryDirectory() as tmp:
        refused = audits_refused(tmp)
    if refused:
        print("the audits refuse to run in a module")
    out = emit_check({MODULE: inject_module(MODULE.read_text()),
                      PROOF: inject_private_import(PROOF.read_text())}, PRIVATE)
    kernel = forged("skip_kernel", ["lake", "env", "leanchecker", "Cralloc.Proof.Selftest.X86_64"],
                    "leanchecker found a problem")
    hijack = forged("audit_macro_hijack", ["lake", "env", "lean", "--run", "Emit.lean", "--check"],
                    "non-standard axiom: VG.Proof.Selftest.X86_64.vg_helper")
    if (subprocess.run(["git", "diff", "--quiet", "--", str(MODULE), str(PROOF)]).returncode != 0
            or PRIVATE.exists()):
        print(f"{MODULE} or {PROOF} was not restored, or {PRIVATE} not deleted", file=sys.stderr)
        return 1
    return 0 if rejected(out) and refused and kernel and hijack else 1


if __name__ == "__main__":
    sys.exit(main())
