#!/usr/bin/env python3
"""Generates the aarch64 kernels: gen.py OUT_DIR writes kernels.s and specs.rs there.

Each family is a module beside this one with a `build(k)` function. `k.kernel` adds one loop.

Registers in a kernel: x19 the loop counter, x20 state (`Kind`), x21 rseq offset, x22 an array,
x23 class offset; x24 to x28 are free and survive calls; x0 to x17 are free and scratch. x18 and
the frame pointer are left alone. Labels 2 and 9 are the loop's start and a trap for what must
not happen; use local labels 3 to 8 or names from `k.label`.
"""
import os
import sys

sys.path.insert(0, os.path.dirname(__file__))

import centry
import core_ops
import floors
import frontend
import memory
import routines


class Kernels:
    def __init__(self):
        self.asm = []
        self.specs = []
        self.names = set()
        self.next_label = 0

    def label(self, stem="l"):
        self.next_label += 1
        return f"rf_{stem}_{self.next_label}"

    def raw(self, text):
        self.asm.append(text)

    def kernel(self, name, body, setup="", fini="", ops=1, note="", kind="Simple", align=64, pad=0,
               outside=""):
        """A loop of `body`. `pad` nops put the loop's first instruction `pad` instructions past
        an `align`-byte boundary. `outside` is text placed after the function."""
        assert name not in self.names, name
        self.names.add(name)
        pad_text = "\n".join(["nop"] * pad)
        self.asm.append(f""".text
.balign 64
.globl {name}
.type {name},%function
{name}:
  PROLOGUE
{setup}
  .balign {align}
{pad_text}
2:
{body}
  subs x19, x19, #1
  b.ne 2b
{fini}
  EPILOGUE
9:
  brk #1
.size {name}, .-{name}
{outside}
""")
        self.specs.append((name, kind, ops, note))


HEADER = r"""
.arch armv8.4-a+lse+rcpc
.macro PROLOGUE
  stp x29, x30, [sp, #-96]!
  stp x19, x20, [sp, #16]
  stp x21, x22, [sp, #32]
  stp x23, x24, [sp, #48]
  stp x25, x26, [sp, #64]
  stp x27, x28, [sp, #80]
  mov x19, x0
  mov x20, x1
  mov x21, x2
  mov x22, x3
  mov x23, x4
.endm
.macro EPILOGUE
  ldp x19, x20, [sp, #16]
  ldp x21, x22, [sp, #32]
  ldp x23, x24, [sp, #48]
  ldp x25, x26, [sp, #64]
  ldp x27, x28, [sp, #80]
  ldp x29, x30, [sp], #96
  ret
.endm

.section .bss
.balign 16384
.globl rf_slabs
rf_slabs:
  .zero (8192 + 16) * 4096
.balign 16384
.globl rf_tables
rf_tables:
  .zero 2097152
.globl rf_abort_count
rf_abort_count:
  .quad 0
.globl rf_data
.balign 128
rf_data:
  .zero 4096
.text
"""


def main():
    out = sys.argv[1]
    k = Kernels()
    k.raw(HEADER)
    for family in (core_ops, memory, frontend, floors, routines, centry):
        family.build(k)
    with open(os.path.join(out, "kernels.s"), "w") as f:
        f.write("\n".join(k.asm))
    with open(os.path.join(out, "specs.rs"), "w") as f:
        f.write('extern "C" {\n')
        for name, _, _, _ in k.specs:
            f.write(f"    fn {name}(n: u64, s: *mut u8, a: u64, r: *mut u8, c: u64) -> u64;\n")
        f.write("}\npub static SPECS: &[Spec] = &[\n")
        for name, kind, ops, note in k.specs:
            esc = note.replace("\\", "\\\\").replace('"', '\\"')
            f.write(f'    Spec {{ name: "{name}", f: {name}, kind: Kind::{kind}, ops: {ops}, note: "{esc}" }},\n')
        f.write("];\n")


if __name__ == "__main__":
    main()
