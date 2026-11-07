#!/usr/bin/env python3
"""Generate the kernels that measure the slab word's store-to-load chain against code placement.

Run by build.rs; writes one Rust source file (a `global_asm!` block, extern declarations and a
table) to the path given as the first argument.

The routines are not copies: their text is read from the allocator's generated files
(`crates/cralloc/src/front/generated/`), so the kernels always run what the Lean generator
emits now. Only the symbols are renamed (the slabs, the size table, the slow paths, which trap
here), and the abort handler counts aborts.

Forms (`FORMS`), each a loop around one or two routines:

  if   inline free (the executable form, direct `__rseq_offset` load), K frees in a row
  ifg  the same, with the shared-library form (`__rseq_offset` through the GOT)
  ia   inline allocation, K in a row
  ip   inline allocation then free of the same object
  td   the drop of `Option<Box<Node>>` as rustc emits it for the tree job (recursive, with the
       inline free after the two calls), dropping a complete tree of 511 nodes
  cf   `c_free` by direct call, K in a row
  cm   `c_malloc` by direct call, K in a row
  cp   `c_malloc` then `c_free`

Diagnostic forms (not in `all`):

  tdx  `td` with the commit stored to the next slot instead of the word: same bytes, no chain
  tdm  `td`'s function dropping 4096 such trees whose nodes miss the caches, as in the job
  sa..se  no allocator code: a short function whose last line holds a short tail (`SYNTH`)

Each form is emitted once per placement in `PLACES`: a byte offset from a 4096-byte boundary.
For the inline forms and `td` it places the code that holds the routine (the loop top, or the
drop function's entry); for the C forms, the routines' entries (the loop stays at offset 0).
Everything else about the copies is identical.

A variant of the routine text can be measured by passing the name of an alternative generated
directory in the environment variable CHAIN_GEN (relative to the repository root), which is how
prototypes of the generator's output are compared with the checked-in one. CHAIN_CLASS sets the
word's position in the slab (`CLASS`). CHAIN_SPLIT=INSN
inserts a taken `jmp` to the next instruction after the routine's first line equal to INSN
(the split-run prototype in docs/research/roofline.md, "The front end sets a cliff by micro-op
count and by lines, not by alignment").
"""
import os
import re
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", "..", ".."))
GEN = os.path.join(ROOT, os.environ.get("CHAIN_GEN", "crates/cralloc/src/front/generated"))

# The tree job's node class (index 3): its word is SLABS + 0x78, bytes 24 to 31 of chunk 3.
# CHAIN_CLASS=14 places it at bytes 16 to 23 instead, where it was before 2026-10-19.
CLASS = int(os.environ.get("CHAIN_CLASS", "15"))
SLOTS = 1024
MASK = 8 * SLOTS - 16
C_SIZE = 100  # (100 + 7) >> 3 = 13: the C size table's entry 13 holds CLASS
K = 512  # operations of a chain between resets of the word
DEPTH = 8  # td: 511 nodes

PLACES = list(range(64)) + [2048 + p for p in range(0, 64, 8)] + [4096 - 64 + p for p in range(0, 64, 4)]
EXTRA = [p for p in PLACES if p >= 64]


def asm_string(src: str, fn: str, executable: bool) -> str:
    """The template of `fn`'s asm!/naked_asm! block, in the cfg branch chosen by `executable`."""
    pat = re.compile(r'(#\[cfg\((not\()?any\(feature = "executable"[^\n]*\)\]\n)?(?:#\[[^\n]*\]\n|///[^\n]*\n)*'
                     r'pub\(crate\) unsafe (?:extern "sysv64" )?fn ' + re.escape(fn) + r'\(.*?asm!\(\s*"((?:[^"\\]|\\.)*)"',
                     re.S)
    found = []
    for m in pat.finditer(src):
        cfg, neg = m.group(1), m.group(2)
        is_exe = bool(cfg) and not neg
        found.append((is_exe if cfg else None, m.group(3)))
    for is_exe, text in found:
        if is_exe is None or is_exe == executable:
            return text.replace('\\"', '"').replace("\\n", "\n").replace("\\\\", "\\")
    raise SystemExit(f"gen.py: no asm for {fn} (executable={executable}) in the generated files")


def read(rel):
    with open(os.path.join(GEN, rel)) as f:
        return f.read()


INLINE = read("x86_64_inline/slab.rs")
CENTRY = read("x86_64/centry.rs")


def instantiate(text: str, miss: str) -> str:
    subs = {
        "{__rseq_offset}": "__rseq_offset",
        "{SLABS}": "ck_slabs",
        "{C_SIZES}": "ck_csizes",
        "{slow}": "ck_trap",
        "{miss}": miss,
    }
    for a, b in subs.items():
        text = text.replace(a, b)
    if "{" in text:
        raise SystemExit(f"gen.py: unsubstituted operand in {text!r}")
    # Count aborts off the hot path; the handler is in .text.unlikely either way.
    text = text.replace("4:\njmp 5b", "4:\ninc qword ptr [rip + ck_aborts]\njmp 5b")
    # Prototype (CHAIN_SPLIT=INSTRUCTION): a taken jump to the next instruction after the first
    # line equal to INSTRUCTION, which ends the front end's straight run there.
    split = os.environ.get("CHAIN_SPLIT")
    if split:
        lines = text.split("\n")
        if split in lines:
            i = lines.index(split)
            lines[i + 1:i + 1] = ["jmp 9f", "9:"]
        text = "\n".join(lines)
    return text


def push(executable=True, miss="ck_trap"):
    return instantiate(asm_string(INLINE, "slab_push", executable), miss)


def pop(executable=True):
    return instantiate(asm_string(INLINE, "slab_pop", executable), "ck_trap")


asm = []
kernels = []  # (name, form, place, ops per inner iteration)

# Kernel arguments: rdi outer iterations, rsi the word's address on this CPU, rdx the word's
# value to reset to after each inner run, rcx an object pointer (or the tree's root slot), r8 the
# inner count. Loop state: r13 outer, r15 word address, r12 reset value, rbx object, rbp inner
# count, r14 inner counter.
PRO = ["push rbx", "push rbp", "push r12", "push r13", "push r14", "push r15",
       "mov r13, rdi", "mov r15, rsi", "mov r12, rdx", "mov rbx, rcx", "mov rbp, r8", "mov r14, r8"]
EPI = ["pop r15", "pop r14", "pop r13", "pop r12", "pop rbp", "pop rbx", "ret"]


def loop_kernel(name, place, op):
    """The loop top at `place` from a page boundary; the entry (prologue) after the loop."""
    text = [".text", ".balign 4096", f".skip {place}, 0xcc", f"{name}_top:"] + op + [
        "dec r14", f"jnz {name}_top",
        "mov qword ptr [r15], r12", "mov r14, rbp",
        "dec r13", f"jnz {name}_top"] + EPI + [
        f".globl {name}", f".type {name},@function", f"{name}:"] + PRO + [f"jmp {name}_top", f".size {name}, .-{name}"]
    asm.append("\n".join(text))


def inline_form(form, place):
    name = f"ck_{form}_{place}"
    word = [f"lea rdi, [rip + ck_slabs + {8 * CLASS}]", f"mov esi, {MASK}"]
    if form in ("if", "ifg"):
        op = word + ["mov rdx, rbx", push(form == "if")]
    elif form == "ia":
        op = word + [pop(), "test rax, rax", "jz ck_trap"]
    elif form == "ip":
        op = word + [pop(), "test rax, rax", "jz ck_trap", "mov rdx, rax"] + word + [push()]
    else:
        raise ValueError(form)
    loop_kernel(name, place, op)
    kernels.append((name, form, place))


def tree_drop(place, form="td"):
    """`drop_in_place::<Option<Box<Node>>>` as rustc emitted it in the tree job (fat LTO, the
    executable form), at entry `place`; the kernel drops the tree once per outer iteration.

    `tdx` is a diagnostic: the commit stores to the slot after the one just written
    (`[rcx + 8]`, the same length) instead of to the word, so no free reads what the previous
    one stored. Same bytes, same placement, no store-to-load chain."""
    fn = f"ck_{form}drop_{place}"
    routine = push(True, f"{fn}_slow")
    if form == "tdx":
        commit = "mov QWORD PTR [rdi+r8*1], rax"
        assert routine.count(commit) == 1, routine
        routine = routine.replace(commit, "mov QWORD PTR [rcx+8], rax")
    body = [".text", ".balign 4096", f".skip {place}, 0xcc", f"{fn}:",
            "push rbx", "mov rbx, qword ptr [rdi]", "test rbx, rbx", f"je {fn}_ret",
            "mov rdi, rbx", f"call {fn}", "lea rdi, [rbx + 8]", f"call {fn}",
            f"lea rdi, [rip + ck_slabs + {8 * CLASS}]", f"mov esi, {MASK}", "mov rdx, rbx",
            routine,
            f"{fn}_ret:", "pop rbx", "ret",
            f"{fn}_slow:", "jmp ck_trap"]
    asm.append("\n".join(body))
    name = f"ck_{form}_{place}"
    text = [".text", ".balign 64", f"{name}_top:", "mov rdi, rbx", f"call {fn}",
            "dec r14", f"jnz {name}_top", "mov qword ptr [r15], r12", "mov r14, rbp",
            "dec r13", f"jnz {name}_top"] + EPI + [
        f".globl {name}", f".type {name},@function", f"{name}:"] + PRO + [f"jmp {name}_top", f".size {name}, .-{name}"]
    asm.append("\n".join(text))
    kernels.append((name, form, place))
    if form == "td":
        # tdm: the same function dropping many trees of the same shape whose nodes lie one per
        # line at shuffled places in a large arena, so that, as in the tree job, the node loads
        # miss the caches. rbx holds the roots' array, rbp their number; the word is reset
        # after each tree.
        name = f"ck_tdm_{place}"
        text = [".text", ".balign 64", f"{name}_top:", "lea rdi, [rbx + r14*8 - 8]", f"call {fn}",
                "mov qword ptr [r15], r12", "dec r14", f"jnz {name}_top", "mov r14, rbp",
                "dec r13", f"jnz {name}_top"] + EPI + [
            f".globl {name}", f".type {name},@function", f"{name}:"] + PRO + [f"jmp {name}_top", f".size {name}, .-{name}"]
        asm.append("\n".join(text))
        kernels.append((name, "tdm", place))


LEAS = ["lea r9, [r10 + 1]"] * 13  # independent, four bytes each
# No allocator at all: small functions called in a loop, at entry `place`, to show what the
# last 64-byte line of a straight run costs in general. (body, edi before each call)
SYNTH = {
    "sa": (LEAS + ["ret"], None),  # 53 bytes
    "sb": (["push rbx"] + LEAS + ["pop rbx", "ret"], None),  # 55 bytes
    # The drop's shape: an early exit to the epilogue, taken on every other call (sc) or never (sd).
    "sc": (["push rbx", "test edi, edi", "je 1f"] + LEAS + ["1:", "pop rbx", "ret"], "mov edi, r14d\nand edi, 1"),
    "sd": (["push rbx", "test edi, edi", "je 1f"] + LEAS + ["1:", "pop rbx", "ret"], "mov edi, 1"),
    "se": (["mov eax, ecx"] * 26 + ["ret"], None),  # two-byte instructions, 53 bytes
}


def synth(form, place):
    body, edi = SYNTH[form]
    fn = f"ck_{form}fn_{place}"
    asm.append("\n".join([".text", ".balign 4096", f".skip {place}, 0xcc", f"{fn}:"] + body))
    name = f"ck_{form}_{place}"
    text = [".text", ".balign 64", f"{name}_top:"] + ([edi] if edi else []) + [f"call {fn}",
            "dec r14", f"jnz {name}_top", "mov qword ptr [r15], r12", "mov r14, rbp",
            "dec r13", f"jnz {name}_top"] + EPI + [
        f".globl {name}", f".type {name},@function", f"{name}:"] + PRO + [f"jmp {name}_top", f".size {name}, .-{name}"]
    asm.append("\n".join(text))
    kernels.append((name, form, place))


def c_routine(fn, place):
    sym = f"ck_{fn}_{place}"
    text = instantiate(asm_string(CENTRY, fn, False), "ck_trap")
    asm.append("\n".join([".text", ".balign 4096", f".skip {place}, 0xcc", f"{sym}:", text]))
    return sym


def c_form(form, place, routines):
    name = f"ck_{form}_{place}"
    m, f = routines
    if form == "cf":
        op = ["mov rdi, rbx", f"call {f}"]
    elif form == "cm":
        op = [f"mov edi, {C_SIZE}", f"call {m}"]
    elif form == "cp":
        op = [f"mov edi, {C_SIZE}", f"call {m}", "mov rdi, rax", f"call {f}"]
    else:
        raise ValueError(form)
    text = [".text", ".balign 64", f"{name}_top:"] + op + [
        "dec r14", f"jnz {name}_top", "mov qword ptr [r15], r12", "mov r14, rbp",
        "dec r13", f"jnz {name}_top"] + EPI + [
        f".globl {name}", f".type {name},@function", f"{name}:"] + PRO + [f"jmp {name}_top", f".size {name}, .-{name}"]
    asm.append("\n".join(text))
    kernels.append((name, form, place))


for place in PLACES:
    for form in ("if", "ifg", "ia", "ip"):
        inline_form(form, place)
    tree_drop(place)
    tree_drop(place, "tdx")
    if place < 64:
        for form in SYNTH:
            synth(form, place)
    r = (c_routine("c_malloc", place), c_routine("c_free", place))
    for form in ("cf", "cm", "cp"):
        c_form(form, place, r)

# The slabs as the C routines expect them: the mirror (shift, base index) at +33555456, the masks'
# table at +33556480 and the region table at +33560576.
data = f"""
.section .bss
.balign 4096
.globl ck_slabs
ck_slabs:
  .zero 33560576 + 4096
.balign 4096
.globl ck_csizes
ck_csizes:
  .zero 2048 + 4097
.balign 64
.globl ck_aborts
ck_aborts:
  .quad 0

.section .text.unlikely, "ax"
.globl ck_trap
ck_trap:
  ud2
.globl ck_fs0
ck_fs0:
  mov rax, qword ptr fs:[0]
  ret
"""

with open(sys.argv[1], "w") as f:
    f.write('std::arch::global_asm!(r#"\n' + data + "\n" + "\n\n".join(asm) + '\n"#);\n')
    f.write('extern "C" {\n')
    for name, *_ in kernels:
        f.write(f"    fn {name}(outer: u64, word: *mut u64, reset: u64, p: u64, inner: u64);\n")
    f.write("}\n")
    f.write(f"pub const CLASS: usize = {CLASS};\npub const SLOTS: usize = {SLOTS};\npub const MASK: u64 = {MASK};\n")
    f.write(f"pub const C_SIZE: usize = {C_SIZE};\npub const K: u64 = {K};\npub const DEPTH: u32 = {DEPTH};\n")
    f.write("pub type Kernel = unsafe extern \"C\" fn(u64, *mut u64, u64, u64, u64);\n")
    f.write("/// (name, kernel, form, placement)\n")
    f.write("pub static KERNELS: &[(&str, Kernel, &str, u32)] = &[\n")
    for name, form, place in kernels:
        f.write(f'    ("{name}", {name}, "{form}", {place}),\n')
    f.write("];\n")
