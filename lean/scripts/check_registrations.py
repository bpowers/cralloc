#!/usr/bin/env python3
"""Checks that the files reviewed by hand only bind (CLAUDE.md, "Adding a
primitive", step 4). Exits non-zero on violations, naming each.

Registration files (`Artifacts/`, `Generic/`) and variant files
(`Variants/`) under lean/Asmalloc/ state what an implementation
chooses and bind proofs made under `Proof/`:

  * no theorems or lemmas;
  * no tactic proofs (`by …`) but the defaults of `Registration`
    (`by sp_safe`, `by of_sig`), which a builder of artifacts may also give
    its own arguments: a side condition a registration would decide is an
    `autoParam` of the lemma that needs it, or the algorithm's proofs state
    the instance as a theorem.
"""

import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
LEAN = ROOT / "lean"

DIRS = ["Artifacts", "Generic", "Variants"]

# The tactics a registration may run: `Registration`'s defaults.
DEFAULTS = {"sp_safe", "of_sig"}

THEOREM = re.compile(r"(?<![\w.'])(theorem|lemma)\s")
BY = re.compile(r"(?<![\w.'])by(?![\w'])\s*(\S*)")


def code_only(text: str) -> str:
    """`text` with its comments and string literals blanked (newlines kept,
    so offsets keep their lines)."""
    out = []
    i, n = 0, len(text)
    while i < n:
        if text.startswith("--", i):
            j = text.find("\n", i)
            j = n if j < 0 else j
            out.append(" " * (j - i))
            i = j
        elif text.startswith("/-", i):
            depth, j = 0, i
            while j < n:
                if text.startswith("/-", j):
                    depth += 1
                    j += 2
                elif text.startswith("-/", j):
                    depth -= 1
                    j += 2
                    if depth == 0:
                        break
                else:
                    j += 1
            out.append(re.sub(r"[^\n]", " ", text[i:j]))
            i = j
        elif text[i] == '"':
            j = i + 1
            while j < n and text[j] != '"':
                j += 2 if text[j] == "\\" else 1
            j += 1
            out.append(re.sub(r"[^\n]", " ", text[i:j]))
            i = j
        else:
            out.append(text[i])
            i += 1
    return "".join(out)


def problems(lean: pathlib.Path) -> list[str]:
    """What is wrong under `lean` (the directory with `Asmalloc/`), one
    line each."""
    out = []
    for d in DIRS:
        for f in sorted((lean / "Asmalloc" / d).rglob("*.lean")):
            rel = f.relative_to(lean).as_posix()
            code = code_only(f.read_text())
            line = lambda i: code.count("\n", 0, i) + 1
            for m in THEOREM.finditer(code):
                out.append(f"lean/{rel}:{line(m.start())}: a {m.group(1)} in a file reviewed by hand; "
                           "prove it under `Proof/` (CLAUDE.md, \"Adding a primitive\")")
            for m in BY.finditer(code):
                if m.group(1).rstrip(")") in DEFAULTS:
                    continue
                out.append(f"lean/{rel}:{line(m.start())}: a tactic proof in a file reviewed by hand; "
                           "make the side condition an `autoParam` of the lemma, or prove the "
                           "instance under `Proof/` (CLAUDE.md, \"Adding a primitive\")")
    return out


def main() -> int:
    ps = problems(LEAN)
    for p in ps:
        print(p)
    return 1 if ps else 0


if __name__ == "__main__":
    sys.exit(main())
