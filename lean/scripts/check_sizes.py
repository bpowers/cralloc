#!/usr/bin/env python3
"""Checks the size and alignment of every naked function the emitter
writes, by assembling it. Exits non-zero on violations, naming each.

The design's rule for a routine emitted as a naked function
(docs/design.md, "Front end"; docs/research/zen5-roofline.md, section 13):
it starts on a 64-byte boundary, and its hot path is at most 64 bytes, or
from 76 to 128: one line crossing costs nothing unless only the last few
instructions spill into the second line, which 65 to 75 bytes would do. The micro-op half of the rule is
`Rust.checkUops`, in Lean; bytes depend on the assembler's encodings, so
they are measured here, on the assembled output.

Each `naked_asm!` template of `<dir>/<target>/*.rs` (by default the
allocator crate's `src/front/generated`) is assembled alone
with `llvm-mc` (the assembler rustc uses), in a section of its own as
rustc places each function with `-ffunction-sections`, its `sym` operands
left as undefined symbols (so a branch to one takes the 32-bit form it
takes when linked). The function's bytes are those of its section up to
its alignment directive (`Printer.funcAlign`), which follows its return:
everything in place, the hot path; the cold code and the descriptor are in
sections of their own (`.text.unlikely`, `__rseq_cs`). Its start is aligned
if its section is: the directive raises the section's alignment.
"""

import pathlib
import re
import shutil
import subprocess
import sys
import tempfile

LEAN = pathlib.Path(__file__).resolve().parent.parent

MAX_BYTES = 128
ONE_LINE = 64
SPILL_END = 75
ALIGN = 64

NAKED = re.compile(
    r'(?:#\[cfg\(([^\n]*)\)\]\n)?#\[unsafe\(naked\)\]\s*pub\(crate\) unsafe extern "[^"]*" fn (\w+)'
    r'.*?core::arch::naked_asm!\(\s*(r#*"|")', re.S)


def rust_string(text: str, start: int, opener: str) -> tuple[str, int]:
    """The value of the Rust string literal whose opening quote ends at
    `start` (`opener` is `"`, `r"` or `r#"`...), and where it ends."""
    if opener.startswith("r"):
        closer = '"' + "#" * (len(opener) - 2)
        end = text.index(closer, start)
        return text[start:end], end + len(closer)
    out, i = [], start
    escapes = {"n": "\n", "t": "\t", '"': '"', "\\": "\\", "'": "'", "0": "\0"}
    while text[i] != '"':
        if text[i] == "\\":
            nxt = text[i + 1]
            if nxt == "\n":
                i += 2
                while text[i] in " \t\n":
                    i += 1
                continue
            out.append(escapes[nxt])
            i += 2
        else:
            out.append(text[i])
            i += 1
    return "".join(out), i + 1


def template_asm(template: str, operands: list[str]) -> str:
    """The template with each `{name}` operand as the symbol `name`, and
    `{{`, `}}` as braces."""
    def sub(m: re.Match) -> str:
        if m.group(0) in ("{{", "}}"):
            return m.group(0)[0]
        name = m.group(1)
        if name not in operands:
            raise ValueError(f"operand {{{name}}} not declared")
        return f"vg_op_{name}"
    return re.sub(r"\{\{|\}\}|\{(\w+)\}", sub, template)


def functions(path: pathlib.Path):
    text = path.read_text()
    for m in NAKED.finditer(text):
        cfg, name, opener = m.group(1), m.group(2), m.group(3)
        template, end = rust_string(text, m.end(), opener)
        close = text.index("\n    )", end)
        operands = re.findall(r"(\w+)\s*=\s*sym\b", text[end:close])
        yield name, cfg, template_asm(template, operands)


def measure(mc: str, name: str, asm: str, tmp: pathlib.Path) -> tuple[int, int]:
    """The bytes of `name`'s code in place, up to its alignment directive,
    and the alignment of its section."""
    lines = asm.rstrip("\n").split("\n")
    if not lines[-1].strip().startswith((".p2align", ".balign", ".align")):
        raise ValueError("does not end with an alignment directive")
    body = "\n".join(lines[:-1])
    src = (f'.intel_syntax noprefix\n.section .text.{name},"ax",@progbits\n'
           f"{name}:\n{body}\n{name}_end:\n{lines[-1]}\n")
    s, o = tmp / f"{name}.s", tmp / f"{name}.o"
    s.write_text(src)
    subprocess.run([mc, "-triple=x86_64-unknown-linux-gnu", "-filetype=obj",
                    "-o", str(o), str(s)], check=True, capture_output=True, text=True)
    syms = subprocess.run(["llvm-nm", str(o)], check=True, capture_output=True,
                          text=True).stdout
    addr = {}
    for line in syms.splitlines():
        parts = line.split()
        if len(parts) == 3:
            addr[parts[2]] = int(parts[0], 16)
    sections = subprocess.run(["llvm-readelf", "-S", "-W", str(o)], check=True,
                              capture_output=True, text=True).stdout
    align = None
    for line in sections.splitlines():
        cols = line.split("]", 1)[-1].split()
        if cols and cols[0] == f".text.{name}":
            align = int(cols[-1])
    if align is None:
        raise ValueError("its section is missing")
    return addr[f"{name}_end"] - addr[name], align


def main() -> int:
    mc = shutil.which("llvm-mc")
    if mc is None or shutil.which("llvm-nm") is None or shutil.which("llvm-readelf") is None:
        print("check_sizes: llvm-mc, llvm-nm and llvm-readelf are needed", file=sys.stderr)
        return 1
    default = LEAN.parent / "crates" / "asmalloc" / "src" / "front" / "generated"
    gen = pathlib.Path(sys.argv[1]) if len(sys.argv) > 1 else default
    bad = []
    count = 0
    with tempfile.TemporaryDirectory() as d:
        tmp = pathlib.Path(d)
        for path in sorted(gen.glob("*/*.rs")):
            for name, cfg, asm in functions(path):
                count += 1
                where = f"{path.relative_to(gen)}: {name}" + (f" [{cfg}]" if cfg else "")
                try:
                    size, align = measure(mc, name, asm, tmp)
                except (ValueError, subprocess.CalledProcessError) as e:
                    msg = e.stderr.strip() if isinstance(e, subprocess.CalledProcessError) else e
                    bad.append(f"{where}: cannot measure: {msg}")
                    continue
                print(f"{where}: {size} bytes, aligned to {align}")
                if size > MAX_BYTES:
                    bad.append(f"{where} is {size} bytes, more than {MAX_BYTES}")
                elif ONE_LINE < size <= SPILL_END:
                    bad.append(f"{where} is {size} bytes: past one line by too little "
                               f"({ONE_LINE + 1} to {SPILL_END})")
                if align < ALIGN:
                    bad.append(f"{where} starts aligned to {align} bytes, not {ALIGN}")
    if count == 0:
        bad.append(f"no naked function found under {gen}")
    for b in bad:
        print(b, file=sys.stderr)
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
