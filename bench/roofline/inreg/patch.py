#!/usr/bin/env python3
"""Rewrite the generated inline pop and push (front/generated/x86_64_inline/slab.rs) so that
their inputs are `in(reg)` operands, which LLVM may place in any register, instead of the fixed
`rdi`, `esi` and `rdx`. The result is `rax` becomes `out` (not `lateout`) in the pop, so that no
input shares it: the pop writes `rax` before it last reads the word (and a restart reads it again).

    patch.py FILE    rewrites FILE in place
"""
import re
import sys

path = sys.argv[1]
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
        assert "rdi" not in chunk.split("core::arch::asm!")[1].split("options")[0], chunk
        n += 1
    out.append(chunk)
assert n == 4, n
open(path, "w").write("".join(out))
