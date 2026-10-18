#!/usr/bin/env python3
"""Rewrite the generated inline pop and push (front/generated/x86_64_inline/slab.rs) so that
their inputs are `in(reg)` operands, which LLVM may place in any register, instead of the fixed
`rdi`, `esi` and `rdx`. The result is `rax` becomes `out` (not `lateout`) in the pop, so that no
input shares it: the pop writes `rax` before it last reads the word (and a restart reads it again).

    patch.py [--onereg] FILE    rewrites FILE in place

With --onereg the slab word's address is also formed once per sequence into r9 (`lea r9,
[word + r8]`), and the load and the commit use `[r9]`: a test of whether the word's hop through
memory is slow because its address is `[word + r8]` with r8 reloaded every sequence.
"""
import re
import sys

onereg = "--onereg" in sys.argv[1:]
path = [a for a in sys.argv[1:] if a != "--onereg"][0]
s = open(path).read()
out, n = [], 0
for chunk in re.split(r"(?=#\[cfg\()", s):
    if "fn slab_pop(" in chunk or "fn slab_push(" in chunk:
        chunk = chunk.replace("[rdi+r8*1]", "[{word}+r8*1]")
        chunk = chunk.replace("test ecx, esi", "test ecx, {mask:e}").replace("test eax, esi", "test eax, {mask:e}")
        chunk = chunk.replace("mov QWORD PTR [rcx], rdx", "mov QWORD PTR [rcx], {p}")
        chunk = chunk.replace('in("rdi") word,', "word = in(reg) word,")
        chunk = chunk.replace('in("rsi") mask,', "mask = in(reg) mask,")
        chunk = chunk.replace('in("rdx") p,', "p = in(reg) p,")
        chunk = chunk.replace('lateout("rax") ret,', 'out("rax") ret,')
        if onereg:
            chunk = chunk.replace("mov rcx, QWORD PTR [{word}+r8*1]",
                                  "lea r9, [{word}+r8*1]\nmov rcx, QWORD PTR [r9]")
            chunk = chunk.replace("QWORD PTR [{word}+r8*1], ", "QWORD PTR [r9], ")
            chunk = chunk.replace('out("r8") _,', 'out("r8") _,\n            out("r9") _,')
            assert "{word}+r8" in chunk and "[r9]" in chunk
        assert "rdi" not in chunk.split("core::arch::asm!")[1].split("options")[0], chunk
        n += 1
    out.append(chunk)
assert n == 4, n
open(path, "w").write("".join(out))
