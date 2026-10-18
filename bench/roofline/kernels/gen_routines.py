#!/usr/bin/env python3
"""Generate the kernels that measure encodings and placements of the proved slab routines.

Run by build.rs; writes one Rust source file (a `global_asm!` block, extern declarations and
tables) to the path given as the first argument. `src/routines.rs` includes it and drives it.

The sequences mirror `crates/asmalloc/src/front/generated/x86_64/slab.rs` (out of line) and
`crates/asmalloc/src/front/generated/x86_64_inline/slab.rs` (inline) instruction for
instruction. Each variant changes one thing, named by its flags:

  short   the two conditional jumps to the slow path are rel8, to a `jmp` stub after the `ret`
  nrx     registers renamed so that 32-bit operations need no REX prefix
  addr    how the slab word's address is formed:
            gen  shl cpu,12; lea s,[SLABS]; lea s,[s+class*8]; [s+cpu]       (as generated)
            mrg  shl cpu,9;  lea s,[SLABS]; add cpu,class;      [s+cpu*8]     (merged)
            idx  shl cpu,12; lea s,[SLABS]; lea cpu,[cpu+class*8]; [s+cpu]    (s stays SLABS)
  mask    where the class's mask comes from:
            tab  lea t,[MAG_MASKS]; test w,[t+class*4]       (as generated: a separate table)
            m16  test w16,[s+class*2-1024]  16-bit table just before SLABS, reached from s
            m8   test w,[s-4096] (or [s+class*8-4096])  table of 8-byte entries just before
                 SLABS, so entry `class` sits 4096 bytes below the class's slab-0 word. Every
                 entry then has the page offset of its class's slab words (4K aliasing).
            m8x  the same table 6144 bytes below, so no entry shares a page offset with a slab
                 word of any class
  sign    `test cpu,cpu; js` in place of `cmp cpu,8192; jae`
  direct  `mov b,[rip+__rseq_offset]` in place of the GOT load and the load through it

Out-of-line routines are emitted once per entry offset 0..63 within a 64-byte line, and once
placed so that the hot path ends on a line boundary. Direct-call pair and batch loops sweep each
routine with the other at offset 0. Inline variants are fixed pair and batch loops. Two kernels
call through registers instead, for comparison.
"""
import sys
from dataclasses import dataclass, replace

CLASS = 3
COFF = CLASS * 8
MASK = 496  # 64-slot magazines: 8 * 64 - 16
SLOW = "rk_slow_trap"


@dataclass(frozen=True)
class V:
    short: bool = False
    nrx: bool = False
    addr: str = "gen"
    mask: str = "tab"
    sign: bool = False
    direct: bool = False
    per_class: bool = False


# Out-of-line variants. Item numbers refer to the questions in docs/research/zen5-roofline.md.
OOL = {
    "gen": V(),
    "genb": V(),  # an identical copy at other addresses: the noise floor
    "sj": V(short=True),
    "b16": V(addr="idx", mask="m16"),
    "b8": V(mask="m8"),
    "nrx": V(nrx=True),
    "mrg": V(addr="mrg"),
    "sv": V(short=True, nrx=True, addr="mrg", mask="m16"),
    "sv8": V(short=True, nrx=True, mask="m8"),
    "sgn": V(sign=True),
    "dir": V(direct=True),
    "sv8s": V(short=True, nrx=True, mask="m8", sign=True),
    "sv8d": V(short=True, nrx=True, mask="m8", direct=True),
    "sv8sd": V(short=True, nrx=True, mask="m8", sign=True, direct=True),
    "b8x": V(mask="m8x"),
    "sv8x": V(short=True, nrx=True, mask="m8x"),
    "sv8xs": V(short=True, nrx=True, mask="m8x", sign=True),
    "sv8xd": V(short=True, nrx=True, mask="m8x", direct=True),
    "sv8xsd": V(short=True, nrx=True, mask="m8x", sign=True, direct=True),
    "svsd": V(short=True, nrx=True, addr="mrg", mask="m16", sign=True, direct=True),
    "pc": V(per_class=True),
    "pcs": V(per_class=True, short=True, nrx=True),
    "pcsd": V(per_class=True, short=True, nrx=True, sign=True, direct=True),
}


def regs(v: V, push: bool):
    """Register roles: rseq base, descriptor temporary, cpu, slab pointer, mask-table pointer."""
    if not v.nrx:
        return dict(base="r10", desc="r11", cpu="r8", cpu32="r8d", slab="r9", tab="r11")
    if v.per_class:
        # No class argument: rsi is free for the slab pointer in push too (p is in rdi).
        return dict(base="rdx", desc="rcx" if push else "rax", cpu="rdx", cpu32="edx", slab="rsi", tab=None)
    if push:
        # rdi = class, rsi = p, rcx = word, rax = new word: only rdx is left without a REX prefix.
        return dict(base="rdx", desc="rcx", cpu="rdx", cpu32="edx", slab="r9", tab="r11")
    return dict(base="rdx", desc="rax", cpu="rdx", cpu32="edx", slab="rsi", tab="rax")


def descriptor():
    """The rseq descriptor and abort handler, as generated, plus an abort counter off the hot path."""
    return [
        "7:",
        '.pushsection __rseq_cs, "aw"',
        ".balign 32",
        "3:",
        ".long 0, 0",
        ".quad 6b, 7b - 6b, 4f",
        ".popsection",
        '.pushsection .text.unlikely, "ax"',
        ".byte 0x0f, 0xb9, 0x3d",
        ".long 0x53053053",
        "4:",
        "inc qword ptr [rip + rf_abort_count]",
        "jmp 5b",
        ".popsection",
    ]


def arm(v: V, r, miss):
    out = ["5:"]
    if v.direct:
        out.append(f"mov {r['base']}, qword ptr [rip + __rseq_offset]")
    else:
        out += [f"mov {r['base']}, qword ptr [rip + __rseq_offset@GOTPCREL]", f"mov {r['base']}, qword ptr [{r['base']}]"]
    out += [f"lea {r['desc']}, [rip + 3f]", f"mov qword ptr fs:[{r['base']} + 8], {r['desc']}", "6:",
            f"mov {r['cpu32']}, dword ptr fs:[{r['base']} + 4]"]
    if v.sign:
        out += [f"test {r['cpu32']}, {r['cpu32']}", f"js {miss}"]
    else:
        out += [f"cmp {r['cpu32']}, 8192", f"jae {miss}"]
    return out


def address(v: V, r, coff=None):
    """Lines that form the slab word's address, the operand naming it, and whether the slab
    register holds SLABS itself (rather than SLABS + class * 8)."""
    c, s = r["cpu"], r["slab"]
    if coff is not None:
        return [f"shl {c}, 12", f"lea {s}, [rip + rk_slabs + {coff}]"], f"[{s} + {c}]", False
    if v.addr == "gen":
        return [f"shl {c}, 12", f"lea {s}, [rip + rk_slabs]", f"lea {s}, [{s} + rdi*8]"], f"[{s} + {c}]", False
    if v.addr == "mrg":
        return [f"shl {c}, 9", f"lea {s}, [rip + rk_slabs]", f"add {c}, rdi"], f"[{s} + {c}*8]", True
    if v.addr == "idx":
        return [f"shl {c}, 12", f"lea {s}, [rip + rk_slabs]", f"lea {c}, [{c} + rdi*8]"], f"[{s} + {c}]", True
    raise ValueError(v.addr)


def mask_test(v: V, r, w32, w16, slab_is_base, before_word=()):
    """The mask test of the word in `w32`; `before_word` are lines that compute it, placed after
    the table address as generated."""
    s = r["slab"]
    if v.per_class:
        return list(before_word) + [f"test {w32}, {MASK}"]
    if v.mask == "tab":
        return [f"lea {r['tab']}, [rip + rk_masks]"] + list(before_word) + [f"test {w32}, dword ptr [{r['tab']} + rdi*4]"]
    if v.mask == "m16":
        assert slab_is_base
        return list(before_word) + [f"test {w16}, word ptr [{s} + rdi*2 - 1024]"]
    if v.mask in ("m8", "m8x"):
        d = 4096 if v.mask == "m8" else 6144
        op = f"[{s} + rdi*8 - {d}]" if slab_is_base else f"[{s} - {d}]"
        return list(before_word) + [f"test {w32}, dword ptr {op}"]
    raise ValueError(v.mask)


def pop_body(v: V, miss):
    r = regs(v, push=False)
    lines, slot, base = address(v, r, COFF if v.per_class else None)
    return (arm(v, r, miss) + lines + [f"mov rcx, qword ptr {slot}"] + mask_test(v, r, "ecx", "cx", base)
            + [f"je {miss}", "mov rax, qword ptr [rcx - 8]", "lea rcx, [rcx - 8]", f"mov qword ptr {slot}, rcx"])


def push_body(v: V, miss, p):
    r = regs(v, push=True)
    lines, slot, base = address(v, r, COFF if v.per_class else None)
    return (arm(v, r, miss) + lines + [f"mov rcx, qword ptr {slot}"]
            + mask_test(v, r, "eax", "ax", base, before_word=["lea rax, [rcx + 8]"])
            + [f"je {miss}", f"mov qword ptr [rcx], {p}", f"mov qword ptr {slot}, rax"])


def fused_uops(lines):
    """Fused uops of straight-line code: one per instruction, a cmp or test fused with the
    conditional jump after it. Labels and directives are not instructions."""
    ins = [l for l in lines if not l.endswith(":") and not l.startswith(".")]
    n = len(ins)
    for a, b in zip(ins, ins[1:]):
        if a.split()[0] in ("cmp", "test") and b.split()[0].startswith("j") and b.split()[0] != "jmp":
            n -= 1
    return n


asm = []
routines = []  # (variant, push, offset, symbol, fused uops to and including ret)


END = -1  # placed so that the hot path ends on a line boundary
MID = -2  # placed so that a line boundary splits the hot path in half


def koff(k):
    return {END: "e", MID: "m"}.get(k, f"{k:02d}")


def ool(name, v: V, push, k):
    """One routine at entry offset k in its 64-byte line, or placed by its size (END, MID); the
    assembler computes that padding from the routine's own labels."""
    sym = f"rk_{name}_{'push' if push else 'pop'}_{koff(k)}"
    miss = "8f" if v.short else (SLOW if not v.per_class else "20f")
    if push:
        body = push_body(v, miss, "rdi" if v.per_class else "rsi")
    else:
        body = pop_body(v, miss)
    hot = body + ["ret"]
    tail = []
    if v.short:
        # The class for a per-class routine's slow path is set in the stub.
        tail = ["8:"]
        if v.per_class:
            tail += (["mov rsi, rdi"] if push else []) + [f"mov edi, {CLASS}"]
        tail.append(f"jmp {SLOW}")
    elif v.per_class:
        tail = ['.pushsection .text.unlikely, "ax"', "20:"]
        tail += (["mov rsi, rdi"] if push else []) + [f"mov edi, {CLASS}", f"jmp {SLOW}", ".popsection"]
    if k == END:
        pad = [f".skip (64 - (({sym}_hot_end - {sym}) % 64)) % 64, 0xcc"]
    elif k == MID:
        pad = [f".skip (128 - (({sym}_hot_end - {sym} + 1) / 2)) % 64, 0xcc"]
    else:
        pad = [f".skip {k}, 0xcc"] if k else []
    text = [".text", ".balign 64"] + pad + [
        f".globl {sym}", f".type {sym},@function", f"{sym}:"]
    text += body
    # The descriptor goes right after the commit, as generated; `ret` follows it.
    text += descriptor() + ["ret", f"{sym}_hot_end:"] + tail + [f".size {sym}, .-{sym}", ".balign 64"]
    asm.append("\n".join(text))
    routines.append((name, push, k, sym, fused_uops(hot)))


for name, v in OOL.items():
    for push in (False, True):
        for k in list(range(64)) + [END, MID]:
            ool(name, v, push, k)

# ---- loop kernels -------------------------------------------------------------------------
# Kernel arguments: rdi iterations, rsi and rdx the pop and push routines (only for the kernels
# that call through registers), rcx the batch array. Loop state lives in callee-saved registers:
# r13 counter, rbx batch array, r14 batch index, r12 and r15 the routines.

kernels = []  # (name, ops per iteration, description, variant, pop offset, push offset, form)
NOMETA = ("", 0, 0, "")


def kernel(name, body, ops, desc, meta):
    head = [".text", ".balign 64", f".globl {name}", f".type {name},@function", f"{name}:",
            "push rbx", "push r12", "push r13", "push r14", "push r15",
            "mov r13, rdi", "mov r12, rsi", "mov r15, rdx", "mov rbx, rcx"]
    tail = ["pop r15", "pop r14", "pop r13", "pop r12", "pop rbx", "ret", f"{name}_trap:", "ud2",
            f".size {name}, .-{name}"]
    asm.append("\n".join(head + body + tail))
    kernels.append((name, ops, desc) + meta)


def pair_loop(name, pop, push, desc, meta=NOMETA):
    kernel(name, [".balign 64", f"{name}_top:"] + pop + push + ["dec r13", f"jnz {name}_top"], 1, desc,
           meta[:3] + ("pair",) if meta[0] else meta)


def batch_loop(name, pop, push, desc, meta=NOMETA):
    body = [".balign 64", f"{name}_top:", "xor r14d, r14d", ".balign 32", f"{name}_a:"] + pop + [
        "mov qword ptr [rbx + r14*8], rax", "inc r14", "cmp r14, 32", f"jne {name}_a",
        "xor r14d, r14d", ".balign 32", f"{name}_f:"] + push + [
        "inc r14", "cmp r14, 32", f"jne {name}_f", "dec r13", f"jnz {name}_top"]
    kernel(name, body, 32, desc, meta[:3] + ("batch",) if meta[0] else meta)


def call_kernels(v, pk, qk):
    """Direct-call pair and batch loops for one variant with pop at pk and push at qk. Generic
    routines take the class in edi (and p in rsi); per-class ones take p in rdi."""
    p, q = f"rk_{v}_pop_{koff(pk)}", f"rk_{v}_push_{koff(qk)}"
    tag = f"{v}_{koff(pk)}_{koff(qk)}"
    if OOL[v].per_class:
        pop = [f"call {p}"]
        pair_push = ["mov rdi, rax", f"call {q}"]
        batch_push = ["mov rdi, qword ptr [rbx + r14*8]", f"call {q}"]
    else:
        pop = [f"mov edi, {CLASS}", f"call {p}"]
        pair_push = [f"mov edi, {CLASS}", "mov rsi, rax", f"call {q}"]
        batch_push = [f"mov edi, {CLASS}", "mov rsi, qword ptr [rbx + r14*8]", f"call {q}"]
    pair_loop(f"gc_{tag}", pop, pair_push, f"call pair {tag}", (v, pk, qk, ""))
    batch_loop(f"gk_{tag}", pop, batch_push, f"call batch {tag}", (v, pk, qk, ""))


# Each routine swept with the other split evenly across two lines (offset 0 is itself a slow
# placement for some sizes, and would lift every cell of the sweep), and a few fixed placements.
for v in OOL:
    for k in range(64):
        call_kernels(v, k, MID)
        call_kernels(v, MID, k)
    for pk, qk in ((MID, MID), (END, END), (0, 0)):
        call_kernels(v, pk, qk)

# Calls through registers, for comparison with the direct calls above.
pair_loop("gx_cpair", [f"mov edi, {CLASS}", "call r12"], [f"mov edi, {CLASS}", "mov rsi, rax", "call r15"],
          "call pair through registers")
batch_loop("gx_cbatch", [f"mov edi, {CLASS}", "call r12"],
           [f"mov edi, {CLASS}", "mov rsi, qword ptr [rbx + r14*8]", "call r15"], "call batch through registers")


# Inline forms. The generated pop reports a miss as 0 through rax and the caller tests it; the
# generated push branches to a label (here the kernel's trap, standing in for the Rust block).
def inline_pop(v: V, call=False):
    r = regs(V(), push=False)
    body = arm(v, r, "20f")
    body += [f"shl r8, 12", f"lea r9, [rip + rk_slabs + {COFF}]", "mov rcx, qword ptr [r9 + r8]",
             f"test ecx, {MASK}", "je 20f" if call else "je 21f",
             "mov rax, qword ptr [rcx - 8]", "lea rcx, [rcx - 8]", "mov qword ptr [r9 + r8], rcx"]
    body += descriptor()
    if call:
        # The slow path is called from inside the block through a stub that preserves every
        # register but rax, so the hot path needs no test of the result.
        body += ['.pushsection .text.unlikely, "ax"', "20:", f"mov ecx, {CLASS}", "call rk_pop_slow_preserving",
                 "jmp 7b", ".popsection"]
    else:
        body += ['.pushsection .text.unlikely, "ax"', "20:", "mov eax, 0", "jmp 7b", "21:", "mov eax, 0",
                 "jmp 7b", ".popsection"]
    return body


def inline_push(v: V, miss):
    r = regs(V(), push=True)
    body = arm(v, r, miss)
    body += ["shl r8, 12", f"lea r9, [rip + rk_slabs + {COFF}]", "mov rcx, qword ptr [r9 + r8]",
             "lea rax, [rcx + 8]", f"test eax, {MASK}", f"je {miss}", "mov qword ptr [rcx], rdi",
             "mov qword ptr [r9 + r8], rax"]
    return body + descriptor()


INLINE = {
    "igen": (V(), False),
    "igenb": (V(), False),  # identical copy: the noise floor
    "isgn": (V(sign=True), False),
    "idir": (V(direct=True), False),
    "isd": (V(sign=True, direct=True), False),
    "icall": (V(), True),
    "icd": (V(direct=True), True),
}
inline_uops = {}


# Class-independent inline forms: the caller passes the address of the class's word in slab 0 in
# `rdi` (`lea rdi, [rip + SLABS + class*8]`, which a constant class folds to), and the mask comes
# from a table of 8-byte entries 2048 bytes below the slabs, at `[rdi - 2048]`; push takes p in
# rsi. Flags: `direct` reads __rseq_offset directly; `hoist` puts the caller's `lea` before the
# loop (what LLVM does with a constant class in a loop), else it runs every iteration; `mreg`
# loads the mask into a register before testing it.
WORD_DISP = -2048


def word_mask_test(w32, mreg, disp=WORD_DISP):
    if mreg == "imm":
        return [f"test {w32}, {MASK}"]
    if mreg in ("top", "arg"):
        return [f"test {w32}, r9d"]
    if mreg:
        return [f"mov r9d, dword ptr [rdi {WORD_DISP}]", f"test {w32}, r9d"]
    return [f"test {w32}, dword ptr [rdi {disp}]"]


def word_pop(v: V, mreg, disp=WORD_DISP):
    r = regs(V(), push=False)
    body = ([f"mov r9d, dword ptr [rdi {WORD_DISP}]"] if mreg == "top" else []) + arm(v, r, "20f")
    body += ["shl r8, 12", "mov rcx, qword ptr [rdi + r8]"] + word_mask_test("ecx", mreg, disp) + [
        "je 21f", "mov rax, qword ptr [rcx - 8]", "lea rcx, [rcx - 8]", "mov qword ptr [rdi + r8], rcx"]
    body += descriptor()
    body += ['.pushsection .text.unlikely, "ax"', "20:", "mov eax, 0", "jmp 7b", "21:", "mov eax, 0",
             "jmp 7b", ".popsection"]
    return body


def word_push(v: V, mreg, miss, disp=WORD_DISP):
    r = regs(V(), push=True)
    body = ([f"mov r9d, dword ptr [rdi {WORD_DISP}]"] if mreg == "top" else []) + arm(v, r, miss)
    body += ["shl r8, 12", "mov rcx, qword ptr [rdi + r8]", "lea rax, [rcx + 8]"] + word_mask_test("eax", mreg, disp) + [
        f"je {miss}", "mov qword ptr [rcx], rsi", "mov qword ptr [rdi + r8], rax"]
    return body + descriptor()


WORD = {
    "wlea": (V(), False, False),
    "wleab": (V(), False, False),  # identical copy: the noise floor
    "whoist": (V(), True, False),
    "wmreg": (V(), False, True),
    "wimm": (V(), False, "imm"),
    "wtop": (V(), False, "top"),
    "wdimm": (V(direct=True), False, "imm"),
    "wdtop": (V(direct=True), False, "top"),
    "wd4k": (V(direct=True), False, False, -4096),
    "wd6k": (V(direct=True), False, False, -6144),
    "w6k": (V(), False, False, -6144),
    "wreg": (V(), False, "arg"),
    "wregh": (V(), True, "arg"),
    "wdreg": (V(direct=True), False, "arg"),
    "wdregh": (V(direct=True), True, "arg"),
    "wdlea": (V(direct=True), False, False),
    "wdhoist": (V(direct=True), True, False),
}
WLEA = f"lea rdi, [rip + rk_slabs + {COFF}]"
for name, (v, hoist, mreg, *dd) in WORD.items():
    disp = dd[0] if dd else WORD_DISP
    setup = [WLEA] + ([f"mov r9d, {MASK}"] if mreg == "arg" else [])
    pre = [] if not hoist else setup
    each = [] if hoist else setup
    pop = word_pop(v, mreg, disp)
    test = ["test rax, rax", f"jz gp_{name}_trap"]
    inline_uops[name] = (fused_uops(each + pop[:pop.index("7:")] + test),
                         fused_uops(word_push(v, mreg, "x", disp)[:word_push(v, mreg, "x", disp).index("7:")]))
    kernel(f"gp_{name}", pre + [".balign 64", f"gp_{name}_top:"] + each + pop + test +
           ["mov rsi, rax"] + each + word_push(v, mreg, f"gp_{name}_trap", disp) +
           ["dec r13", f"jnz gp_{name}_top"], 1, f"inline pair: {name}", NOMETA)
    kernel(f"gb_{name}", pre + [".balign 64", f"gb_{name}_top:", "xor r14d, r14d", ".balign 32", f"gb_{name}_a:"]
           + each + word_pop(v, mreg, disp) + ["test rax, rax", f"jz gb_{name}_trap",
           "mov qword ptr [rbx + r14*8], rax", "inc r14", "cmp r14, 32", f"jne gb_{name}_a",
           "xor r14d, r14d", ".balign 32", f"gb_{name}_f:"] + each +
           ["mov rsi, qword ptr [rbx + r14*8]"] + word_push(v, mreg, f"gb_{name}_trap", disp) +
           ["inc r14", "cmp r14, 32", f"jne gb_{name}_f", "dec r13", f"jnz gb_{name}_top"],
           32, f"inline batch: {name}", NOMETA)

for name, (v, call) in INLINE.items():
    pop = inline_pop(v, call)
    push = inline_push(v, f"gp_{name}_trap")
    test = [] if call else ["test rax, rax", f"jz gp_{name}_trap"]
    inline_uops[name] = (fused_uops(pop[:pop.index("7:")] + test), fused_uops(push[:push.index("7:")]))
    pair_loop(f"gp_{name}", pop + test, ["mov rdi, rax"] + push, f"inline pair: {name}")
    batch_loop(f"gb_{name}", pop + ([] if call else ["test rax, rax", f"jz gb_{name}_trap"]),
               ["mov rdi, qword ptr [rbx + r14*8]"] + inline_push(v, f"gb_{name}_trap"), f"inline batch: {name}")

data = r"""
.section .bss
.balign 64
.globl rk_masks
rk_masks:
  .zero 2048
.balign 4096
  .zero 2048
.globl rk_masks8x
rk_masks8x:
  .zero 2048
.globl rk_masks8
rk_masks8:
  .zero 3072
.globl rk_masks16
rk_masks16:
  .zero 1024
.globl rk_slabs
rk_slabs:
  .zero 8192 * 4096

.section .text.unlikely, "ax"
.globl rk_slow_trap
rk_slow_trap:
  ud2

/* Preserves every register but rax: what an in-block call of the slow path needs. The class is
   in ecx. In the kernels the slow path is never taken; this exists so the call assembles. */
.globl rk_pop_slow_preserving
rk_pop_slow_preserving:
  ud2
"""

with open(sys.argv[1], "w") as f:
    f.write('std::arch::global_asm!(r#"\n' + data + "\n" + "\n\n".join(asm) + '\n"#);\n')
    f.write('extern "C" {\n')
    for name, *_ in kernels:
        f.write(f"    fn {name}(iters: u64, pop: *mut u8, push: u64, arr: *mut u8, class: u64) -> u64;\n")
    for _, _, _, sym, _ in routines:
        f.write(f"    fn {sym}();\n    static {sym}_hot_end: u8;\n")
    f.write("}\n")
    f.write("/// (name, kernel, ops per iteration, description, variant, pop offset, push offset, form);\n")
    f.write("/// an offset of -1 means placed to end on a line boundary, -2 split in half by one\n")
    f.write("pub static KERNELS: &[(&str, crate::kernels::Kernel, u64, &str, &str, i32, i32, &str)] = &[\n")
    for name, ops, desc, v, pk, qk, form in kernels:
        f.write(f'    ("{name}", {name}, {ops}, "{desc}", "{v}", {pk}, {qk}, "{form}"),\n')
    f.write("];\n")
    f.write("/// (variant, is push, entry offset or -1 or -2, entry, end of the hot path, fused uops to the ret)\n")
    f.write("pub fn routines() -> Vec<(&'static str, bool, i32, *const u8, *const u8, u32)> {\n    vec![\n")
    for name, push, k, sym, uops in routines:
        f.write(f'        ("{name}", {str(push).lower()}, {k}, {sym} as *const u8, &raw const {sym}_hot_end, {uops}),\n')
    f.write("    ]\n}\n")
    f.write("/// (inline variant, fused uops of the pop with the caller's test, of the push)\n")
    f.write("pub static INLINE_UOPS: &[(&str, u32, u32)] = &[\n")
    for name, (a, b) in inline_uops.items():
        f.write(f'    ("{name}", {a}, {b}),\n')
    f.write("];\n")
