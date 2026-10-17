#!/usr/bin/env python3
"""Generate the C-entry kernels: candidate `malloc` and `free` routines and the loops that call them.

    gen.py OUTDIR

writes OUTDIR/routines_lib.S (the routines, prefix L_, for the shared library), routines_exe.S
(the same routines, prefix X_, linked into the benchmark executable), kernels.S (loops in the
executable that call them), front.h and table.h (what the driver needs to know about both), and
standin_*.S (the routines a preloadable stand-in allocator exports as malloc, free and calloc;
see standin.c).

Every routine runs a real restartable sequence against glibc's registered rseq area, like the
routines in lean/generated/. The slow paths are traps (free(NULL) returns), so a kernel that
leaves the fast path fails loudly instead of measuring something else.

Malloc variants take the size in rdi and return the object in rax. Free variants take the pointer
in rdi. Both keep rdi intact on the way to the slow path, which receives the C arguments. Flags:

  struct  fused   one routine: size or address to class, then the pop or push
          rust    what rustc makes of `extern "C" fn malloc` / `free` that compute the class
                  and tail-call the generic out-of-line pop or push (class in edi)
  idx     malloc: how the size becomes a table index
            s  (n + 7) >> 3, sizes up to 1024
            d  n itself, sizes up to 1024 (a 1025-byte table, no shift)
            t  (n + 7) >> 3 up to 1024, else (n + 127 + (120 << 7)) >> 7, up to 32 KiB (cmov)
            k  computed with bsr and shifts, no table, up to 32 KiB
            l  (n + 7) >> 3, a 4097-byte table, up to 32 KiB
            q  (n + 7) >> 3 into a table of 8-byte entries that hold SLABS + 8 * class itself
  addr    lea  the slab word at [r9 + r8], r9 = SLABS + 8 * class formed before the arm
          idx  the word at [r8 + class*8], r8 = SLABS + (cpu << 12) formed inside the sequence
  bias    free: the region byte read through a pointer biased by base >> shift, so the subtract
          feeds only the range check
  reg, maskt, r9   free: bisection knobs that override `tab` for the region table, the mask
          table, and where the slab-word pointer is formed (pre: before the arm, in: inside)
  look   free: how the address becomes a region index
            w2 one word base|shift (as SpaceCell publishes it): shrx p, then shrx w, subtract
            cl the same with `shr r, cl` (no BMI2)
            sb two statics, the shift and base >> shift: shrx, then subtract from memory
  tab     sep   each table is its own symbol, reached with its own lea
          slab  the tables sit at fixed offsets below SLABS, reached from the slab base register
  mask    class the mask table is indexed by class (one more dependent load after the class)
          size  malloc: a packed per-size table gives class offset and mask (both from one index)
  check   x     free: an explicit small-class test after the region table load
          fold  free: the mask table has 256 entries, 0 for every byte that is not a small class,
                so the mask test sends those to the slow path
  data    h  own data by hidden RIP-relative symbols   g  through the GOT
  rseq    got  __rseq_offset through the GOT (what a shared library must do)
          own  a hidden copy of it in our own data (one load; needs initialising first)
  short   conditional jumps to the slow path are rel8, to a stub after the ret
  null    free: `test rdi, rdi; jz ret` first

Also generated: calloc routines (cD, cS, cT: the product, a malloc body, then a jump to a zeroing
tail), the Rust comparisons (generic and per-class pop and push as lean/generated has them, and
fZ, a free told the size), probes (x*, y*: what slows `call [mem]`), free(NULL) loops (kn_*) and
load-latency chases by addressing mode (kl_*). Kernel names: kp_ pair, km_ mixed sizes, kb8_ and
kb32_ bursts, then malloc_free_call, the call d direct, p PLT, g `call [GOT]`, r `call reg` after
a GOT load, R `call reg` hoisted.
"""
import os
import sys
from dataclasses import dataclass, replace

SMALL = 44  # small classes 1..44
CLASS = 4  # the class of malloc(32), used by the Rust comparisons
MASK = 2032  # 256-slot magazines
SLAB_SHIFT = 12

# Offsets of the tables below SLABS (the `slab` placement). Masks sit 6144 below so that no mask
# shares a page offset with a slab word (section 13's m8x); the size tables fill the page below
# SLABS; the region table the page below the masks.
OFF_REGION = -12288  # 4096 bytes
OFF_MASK = -6144  # 256 x 8 bytes, entry c at OFF_MASK + 8c
OFF_SIZE_S = -4096  # 129 bytes
OFF_SIZE_T = -3840  # 377 bytes
OFF_SIZE_D = -3328  # 1025 bytes
OFF_SIZE_L = -20480  # 4097 bytes: (n + 7) >> 3 for every small size

RSEQ_SIG = "0x53053053"


@dataclass(frozen=True)
class M:
    addr: str = "lea"
    struct: str = "fused"
    idx: str = "s"
    tab: str = "slab"
    mask: str = "class"
    data: str = "h"
    rseq: str = "got"
    short: bool = False


@dataclass(frozen=True)
class F:
    addr: str = "lea"
    bias: bool = False
    struct: str = "fused"
    look: str = "w2"
    tab: str = "slab"
    check: str = "fold"
    data: str = "h"
    rseq: str = "got"
    short: bool = False
    null: bool = False
    # Bisection knobs (None: follow `tab`): where the region table is read from, where the mask
    # table is read from, and whether the slab-word pointer is formed before the arm or inside.
    reg: str = None
    maskt: str = None
    r9: str = None


MALLOC = {
    "mR": M(struct="rust"),
    "mA": M(tab="sep"),
    "mS": M(),
    "mD": M(idx="d"),
    "mP": M(tab="sep", mask="size"),
    "mT": M(idx="t"),
    "mK": M(idx="k"),
    "mSg": M(data="g"),
    "mSo": M(rseq="own"),
    "mSj": M(short=True),
    "mDj": M(idx="d", short=True),
    "mTj": M(idx="t", short=True),
    "mQ": M(idx="q"),
    "mQj": M(idx="q", short=True),
    "mL": M(idx="l"),
    "mLj": M(idx="l", short=True),
    "mY": M(addr="idx"),
    "mSb": M(),  # identical copies at other addresses: the noise floor
    "mAb": M(tab="sep"),
    "mYj": M(addr="idx", short=True),
    "mYdj": M(addr="idx", idx="d", short=True),
    "mYtj": M(addr="idx", idx="t", short=True),
}

FREE = {
    "fR": F(struct="rust"),
    "fA": F(tab="sep", check="x"),
    "fB": F(tab="sep"),
    "fS": F(),
    "fSx": F(check="x"),
    "fC": F(look="cl"),
    "fW": F(look="sb"),
    "fSn": F(null=True),
    "fSg": F(data="g"),
    "fSo": F(rseq="own"),
    "fSj": F(short=True),
    "fWj": F(look="sb", short=True),
    "fWjn": F(look="sb", short=True, null=True),
    "fY": F(addr="idx"),
    "fSb": F(),
    "fBb": F(tab="sep"),
    "fB1": F(tab="sep", reg="slab"),
    "fB2": F(tab="sep", maskt="slab"),
    "fB3": F(tab="sep", r9="pre"),
    "fS1": F(reg="sep"),
    "fS2": F(maskt="sep"),
    "fS3": F(r9="in"),
    # Every load on the pointer's path in a 4-cycle form ([base + index], no displacement).
    "fU": F(reg="sep"),
    "fUj": F(reg="sep", short=True),
    "fUjn": F(reg="sep", short=True, null=True),
    "fUw": F(reg="sep", look="sb"),
    "fV": F(bias=True),
    "fVj": F(bias=True, short=True),
    "fVw": F(bias=True, look="sb"),
    "fYj": F(addr="idx", short=True),
    "fYb": F(addr="idx", bias=True),
    "fYbj": F(addr="idx", bias=True, short=True),
    "fYwj": F(addr="idx", look="sb", short=True),
    "fYwbj": F(addr="idx", look="sb", bias=True, short=True),
}


def addr_of(P, sym, data, reg):
    """Lines that put the address of our own symbol in reg."""
    if data == "g":
        return [f"mov {reg}, qword ptr [rip + {P}{sym}_pub@GOTPCREL]"]
    return [f"lea {reg}, [rip + {P}{sym}]"]


def load_word(P, sym, data, reg):
    """Lines that load the 64-bit word at our own symbol into reg."""
    if data == "g":
        return [f"mov {reg}, qword ptr [rip + {P}{sym}_pub@GOTPCREL]", f"mov {reg}, qword ptr [{reg}]"]
    return [f"mov {reg}, qword ptr [rip + {P}{sym}]"]


def arm(P, rseq, data, miss):
    """Arm the sequence and read the CPU number: r10 rseq offset, r11 descriptor, r8 CPU."""
    out = ["5:"]
    if rseq == "got":
        out += ["mov r10, qword ptr [rip + __rseq_offset@GOTPCREL]", "mov r10, qword ptr [r10]"]
    else:
        out += load_word(P, "rseq_off", data, "r10")
    out += ["lea r11, [rip + 3f]", "mov qword ptr fs:[r10 + 8], r11", "6:",
            "mov r8d, dword ptr fs:[r10 + 4]", "cmp r8d, 8192", f"jae {miss}", f"shl r8, {SLAB_SHIFT}"]
    return out


def descriptor(P):
    return [
        "7:",
        '.pushsection __rseq_cs, "aw"', ".balign 32", "3:", ".long 0, 0", ".quad 6b, 7b - 6b, 4f", ".popsection",
        '.pushsection .text.unlikely, "ax"', f".long {RSEQ_SIG}", "4:",
        f"inc qword ptr [rip + {P}abort_count]", "jmp 5b", ".popsection",
    ]


def malloc_fused(P, m: M):
    miss = "8f" if m.short else f"{P}malloc_slow"
    limit = 1024 if m.idx in "sd" else 32768
    L = [f"cmp rdi, {limit}", f"ja {miss}"]
    # Registers that live across the sequence (restart re-runs from label 5): rdi, rdx (class),
    # r9 when the slab base is formed before the arm, rsi (packed entry index).
    if m.idx == "q":
        # The table entry is the register value itself: SLABS + 8 * class (8-byte entries).
        L += ["lea eax, [rdi + 7]", "shr eax, 3"] + addr_of(P, "sizeptr", m.data, "rcx")
        L += ["mov r9, qword ptr [rcx + rax*8]"]
        L += arm(P, m.rseq, m.data, miss)
        L += ["mov rcx, qword ptr [r9 + r8]", f"test ecx, dword ptr [r9 + {OFF_MASK}]", f"je {miss}",
              "mov rax, qword ptr [rcx - 8]", "lea rcx, [rcx - 8]", "mov qword ptr [r9 + r8], rcx"]
        return L, (["8:", f"jmp {P}malloc_slow"] if m.short else [])
    if m.tab == "slab":
        L += addr_of(P, "slabs", m.data, "r9")
    if m.idx == "s":
        L += ["lea eax, [rdi + 7]", "shr eax, 3"]
        ix, off, sym = "rax", OFF_SIZE_S, "size_s"
    elif m.idx == "l":
        L += ["lea eax, [rdi + 7]", "shr eax, 3"]
        ix, off, sym = "rax", OFF_SIZE_L, "size_l"
    elif m.idx == "d":
        ix, off, sym = "rdi", OFF_SIZE_D, "size_d"
    elif m.idx == "t":
        L += ["lea eax, [rdi + 7]", "shr eax, 3", "lea ecx, [rdi + 127 + 15360]", "shr ecx, 7", "cmp rdi, 1024",
              "cmova eax, ecx"]
        ix, off, sym = "rax", OFF_SIZE_T, "size_t"
    elif m.idx == "k":
        # C class of n <= 32768 without a table. n' = 8 for n <= 8, else n rounded up to 16;
        # w = n'/8 - 1; class = w + 1 if w < 8, else 4b + ((w >> (b - 2)) & 3) - 3, b = bsr w.
        L += ["lea rax, [rdi + 15]", "and eax, -16", "mov ecx, 8", "cmp rdi, 8", "cmovbe eax, ecx",
              "shr eax, 3", "lea edx, [rax - 1]", "bsr ecx, edx", "lea esi, [rcx - 2]", "shrx eax, edx, esi",
              "and eax, 3", "lea eax, [rax + rcx*4 - 3]", "inc edx", "cmp edx, 8", "cmovbe eax, edx",
              "mov edx, eax"]
        ix = None
    else:
        raise ValueError(m.idx)
    if m.mask == "size":
        assert m.tab == "sep" and m.idx == "s"
        L += ["mov esi, eax"] + addr_of(P, "packed", m.data, "rdx") + ["mov ecx, dword ptr [rdx + rsi*8]"]
        L += addr_of(P, "slabs", m.data, "r9") + ["add r9, rcx"]
    elif ix is not None:
        if m.tab == "slab":
            L += [f"movzx edx, byte ptr [r9 + {ix} + {off}]"]
        else:
            L += addr_of(P, sym + "_sep", m.data, "rcx") + [f"movzx edx, byte ptr [rcx + {ix}]"]
    # addr lea: r9 = SLABS + 8 class before the arm, the word at [r9 + r8]. addr idx: r8 = SLABS +
    # (cpu << 12) inside the sequence, the word at [r8 + class*8], so the class reaches the
    # address without an instruction of its own.
    if m.tab == "slab" and m.addr == "lea":
        L += ["lea r9, [r9 + rdx*8]"]
    L += arm(P, m.rseq, m.data, miss)
    if m.tab == "sep" and m.mask == "class":
        L += addr_of(P, "slabs", m.data, "r9") + ["lea r9, [r9 + rdx*8]"]
    if m.addr == "idx":
        assert m.tab == "slab"
        L += ["add r8, r9"]
        word = "[r8 + rdx*8]"
    else:
        word = "[r9 + r8]"
    L += [f"mov rcx, qword ptr {word}"]
    if m.mask == "size":
        L += ["test ecx, dword ptr [rdx + rsi*8 + 4]"]
    elif m.tab == "slab":
        L += [f"test ecx, dword ptr [r9 + {OFF_MASK}]" if m.addr == "lea" else
              f"test ecx, dword ptr [r9 + rdx*8 + {OFF_MASK}]"]
    else:
        L += addr_of(P, "masks4", m.data, "r11") + ["test ecx, dword ptr [r11 + rdx*4]"]
    L += [f"je {miss}", "mov rax, qword ptr [rcx - 8]", "lea rcx, [rcx - 8]", f"mov qword ptr {word}, rcx"]
    return L, (["8:", f"jmp {P}malloc_slow"] if m.short else [])


def free_fused(P, f: F):
    miss = "8f" if f.short else f"{P}free_slow"
    reg = f.reg or f.tab
    maskt = f.maskt or f.tab
    r9 = f.r9 or ("pre" if f.tab == "slab" else "in")
    L = []
    if f.null:
        L += ["test rdi, rdi", "jz 9f"]
    if f.look == "w2":
        L += load_word(P, "space", f.data, "rcx") + ["shrx rax, rdi, rcx", "shrx rcx, rcx, rcx", "sub rax, rcx"]
    elif f.look == "cl":
        L += load_word(P, "space", f.data, "rcx") + ["mov rax, rdi", "shr rax, cl", "shr rcx, cl", "sub rax, rcx"]
    elif f.look == "sb":
        if f.data == "g":
            L += load_word(P, "shift", "g", "rcx") + ["shrx rax, rdi, rcx"]
            L += [f"mov rcx, qword ptr [rip + {P}baseidx_pub@GOTPCREL]", "sub rax, qword ptr [rcx]"]
        else:
            L += [f"mov ecx, dword ptr [rip + {P}shift]", "shrx rax, rdi, rcx", f"sub rax, qword ptr [rip + {P}baseidx]"]
    else:
        raise ValueError(f.look)
    if f.bias:
        # The region byte is read at regbias + (p >> shift), where regbias is the table's
        # address minus base >> shift: the subtraction feeds only the range check's branch,
        # not the load. The check runs first, so the load is always inside the table.
        assert f.look in ("w2", "sb") and f.data == "h"
        L[-1:] = []  # rax = p >> shift stays; the subtraction goes into rdx instead
        if f.look == "w2":
            L += ["mov rdx, rax", "sub rdx, rcx"]
        else:
            L += ["mov rdx, rax", f"sub rdx, qword ptr [rip + {P}baseidx]"]
        L += ["cmp rdx, 4096", f"jae {miss}"] + load_word(P, "regbias", f.data, "rcx")
        L += ["movzx edx, byte ptr [rcx + rax]"]
    else:
        L += ["cmp rax, 4096", f"jae {miss}"]
        if reg == "slab":
            L += addr_of(P, "slabs", f.data, "r9") + [f"movzx edx, byte ptr [r9 + rax + {OFF_REGION}]"]
        else:
            L += addr_of(P, "regclass_sep", f.data, "rcx") + ["movzx edx, byte ptr [rcx + rax]"]
    if f.check == "x":
        L += ["lea eax, [rdx - 1]", f"cmp eax, {SMALL}", f"jae {miss}"]
    if r9 == "pre":
        if reg != "slab" or f.bias:
            L += addr_of(P, "slabs", f.data, "r9")
        if f.addr == "lea":
            L += ["lea r9, [r9 + rdx*8]"]
    L += arm(P, f.rseq, f.data, miss)
    if r9 == "in":
        L += addr_of(P, "slabs", f.data, "r9") + (["lea r9, [r9 + rdx*8]"] if f.addr == "lea" else [])
    if f.addr == "idx":
        L += ["add r8, r9"]
        word = "[r8 + rdx*8]"
        mask = f"[r9 + rdx*8 + {OFF_MASK}]"
    else:
        word = "[r9 + r8]"
        mask = f"[r9 + {OFF_MASK}]"
    L += [f"mov rcx, qword ptr {word}", "lea rax, [rcx + 8]"]
    if maskt == "slab":
        L += [f"test eax, dword ptr {mask}"]
    else:
        L += addr_of(P, "masks4", f.data, "r11") + ["test eax, dword ptr [r11 + rdx*4]"]
    L += [f"je {miss}", "mov qword ptr [rcx], rdi", f"mov qword ptr {word}, rax"]
    return L, (["8:", f"jmp {P}free_slow"] if f.short else [])


def calloc_fused(P, m: M):
    """calloc(n, size): the product with an overflow check, the malloc body on it, then a jump to
    `calloc_zero(p, total)`, which zeroes and returns p. A miss goes to `calloc_slow(total)`."""
    body, _ = malloc_fused(P, replace(m, short=True))
    body = ["mov rax, rdi", "mul rsi", "jo 10f", "mov rdi, rax"] + body
    return body, ["8:", f"jmp {P}calloc_slow", "10:", "mov rdi, -1", f"jmp {P}calloc_slow"]


def free_sized(P):
    """A free told the size (rsi), as Rust's dealloc is: the class comes from the size table, not
    from the address. The C classes stand in for Rust's here."""
    L = ["cmp rsi, 1024", f"ja {P}free_slow"] + addr_of(P, "slabs", "h", "r9")
    L += ["lea eax, [rsi + 7]", "shr eax, 3", f"movzx edx, byte ptr [r9 + rax + {OFF_SIZE_S}]", "lea r9, [r9 + rdx*8]"]
    L += arm(P, "got", "h", f"{P}free_slow")
    L += ["mov rcx, qword ptr [r9 + r8]", "lea rax, [rcx + 8]", f"test eax, dword ptr [r9 + {OFF_MASK}]",
          f"je {P}free_slow", "mov qword ptr [rcx], rdi", "mov qword ptr [r9 + r8], rax"]
    return L


def malloc_rust(P, m: M):
    # rustc 1.99 for `if size <= 1024 { pop(SIZE2CLASS[(size + 7) >> 3]) } else { slow(size) }`.
    L = ["cmp rdi, 1025", f"jae {P}malloc_slow", "add rdi, 7", "shr rdi, 3"]
    L += addr_of(P, "size_s_sep", m.data, "rax") + ["movzx edi, byte ptr [rax + rdi]", f"jmp {P}pop_gen_h"]
    return L


def free_rust(P, f: F):
    # rustc 1.99 (target-cpu with BMI2) for the region lookup, a small-class test and a tail call.
    L = load_word(P, "space", f.data, "rcx") + ["mov rsi, rdi", "shrx rax, rdi, rcx", "shrx rcx, rcx, rcx",
                                                 "sub rax, rcx", "cmp rax, 4096", f"jae {P}free_slow"]
    L += addr_of(P, "regclass_sep", f.data, "rcx") + ["movzx edi, byte ptr [rcx + rax]", "lea eax, [rdi - 1]",
                                                       f"cmp al, {SMALL - 1}", "ja 8f", f"jmp {P}push_gen_h"]
    return L, ["8:", "mov rdi, rsi", f"jmp {P}free_slow"]


def pop_gen(P):
    """lean/generated/x86_64/slab.rs `slab_pop`, instruction for instruction (class in edi)."""
    L = arm(P, "got", "h", f"{P}pop_slow")
    L += [f"lea r9, [rip + {P}slabs]", "lea r9, [r9 + rdi*8]", "mov rcx, qword ptr [r9 + r8]",
          f"lea r11, [rip + {P}masks4]", "test ecx, dword ptr [r11 + rdi*4]", f"je {P}pop_slow",
          "mov rax, qword ptr [rcx - 8]", "lea rcx, [rcx - 8]", "mov qword ptr [r9 + r8], rcx"]
    return L


def push_gen(P):
    L = arm(P, "got", "h", f"{P}push_slow")
    L += [f"lea r9, [rip + {P}slabs]", "lea r9, [r9 + rdi*8]", "mov rcx, qword ptr [r9 + r8]",
          f"lea r11, [rip + {P}masks4]", "lea rax, [rcx + 8]", "test eax, dword ptr [r11 + rdi*4]",
          f"je {P}push_slow", "mov qword ptr [rcx], rsi", "mov qword ptr [r9 + r8], rax"]
    return L


def pop_class(P):
    """The inline pop of lean/generated/x86_64_inline with a ret: the Rust free of a constant class."""
    L = arm(P, "got", "h", f"{P}pop_slow")
    L += [f"lea r9, [rip + {P}slabs + {CLASS * 8}]", "mov rcx, qword ptr [r9 + r8]", f"test ecx, {MASK}",
          f"je {P}pop_slow", "mov rax, qword ptr [rcx - 8]", "lea rcx, [rcx - 8]", "mov qword ptr [r9 + r8], rcx"]
    return L


def push_class(P):
    L = arm(P, "got", "h", f"{P}push_slow")
    L += [f"lea r9, [rip + {P}slabs + {CLASS * 8}]", "mov rcx, qword ptr [r9 + r8]", "lea rax, [rcx + 8]",
          f"test eax, {MASK}", f"je {P}push_slow", "mov qword ptr [rcx], rdi", "mov qword ptr [r9 + r8], rax"]
    return L


def fused_uops(lines):
    """Fused uops of straight-line code: one per instruction, a cmp or test fused with the
    conditional jump after it (the definition section 6 of the roofline note uses)."""
    ins = [l for l in lines if not l.endswith(":") and not l.startswith(".")]
    n = len(ins)
    for a, b in zip(ins, ins[1:]):
        if a.split()[0] in ("cmp", "test") and b.split()[0].startswith("j") and b.split()[0] != "jmp":
            n -= 1
    return n


def hot_path(lines):
    """The instructions from entry to the ret on the fast path: everything outside push/popsection."""
    out, depth = [], 0
    for l in lines:
        if l.startswith(".pushsection"):
            depth += 1
        elif l.startswith(".popsection"):
            depth -= 1
        elif depth == 0:
            out.append(l)
    return out


# (name, kind, symbol suffix, body lines without ret, tail after ret, has rseq)
ROUTINES = []


EPILOGUE = {}


def add(name, kind, body, tail=(), seq=True, epilogue=None):
    """A routine: `body`, then (if `seq`) the rseq descriptor, then `epilogue` or a ret, then
    `tail` (cold stubs after the hot path)."""
    ROUTINES.append((name, kind, list(body), list(tail), seq))
    if epilogue:
        EPILOGUE[name] = epilogue


add("pop_gen", "pop", pop_gen("{P}"))
add("push_gen", "push", push_gen("{P}"))
add("pop_cls", "pop", pop_class("{P}"))
add("push_cls", "push", push_class("{P}"))
add("mE", "malloc", ["mov rax, rdi"], seq=False)
# Probes for what slows `call [mem]`: callees with a long ALU chain, a load chain, a store whose
# address comes at the end of a load chain, and a store whose data does. None feeds the result.
add("xA", "malloc", ["mov rax, rdi", "mov rcx, rdi"] + ["imul rcx, rcx"] * 6, seq=False)
add("xL", "malloc", ["mov rax, rdi", "mov rcx, qword ptr [rip + {P}selfp]", "mov rcx, qword ptr [rcx]",
                     "mov rcx, qword ptr [rcx]"], seq=False)
add("xS", "malloc", ["mov rax, rdi", "mov rcx, qword ptr [rip + {P}selfp]", "mov rcx, qword ptr [rcx]",
                     "mov rcx, qword ptr [rcx]", "mov qword ptr [rcx + 8], rdi"], seq=False)
RS = ["mov rcx, qword ptr [rip + __rseq_offset@GOTPCREL]", "mov rcx, qword ptr [rcx]"]
add("xF", "malloc", ["mov rax, rdi"] + RS + ["mov edx, dword ptr fs:[rcx + 4]"], seq=False)
add("xG", "malloc", ["mov rax, rdi"] + RS + ["mov qword ptr fs:[rcx + 24], 0"], seq=False)
add("xH", "malloc", ["mov rax, rdi"] + RS + ["mov rdx, qword ptr fs:[0]", "mov qword ptr [rdx + rcx + 24], 0"],
    seq=False)
add("xI", "malloc", ["mov rax, rdi"] + RS + ["mov rdx, qword ptr fs:[0]", "mov edx, dword ptr [rdx + rcx + 4]"],
    seq=False)
# Size probes: n one-byte nops (one line), n four-byte leas (crossing lines), and a not-taken
# compare-and-branch every four instructions.
for n in (4, 8, 12, 16, 20):
    add(f"xN{n}", "malloc", ["mov rax, rdi"] + ["nop"] * n, seq=False)
    add(f"xW{n}", "malloc", ["mov rax, rdi"] + ["lea rdx, [rdx + rcx*4 + 0x10]"] * n, seq=False)
    add(f"xB{n}", "malloc", ["mov rax, rdi"] + (["cmp rdi, 1", "jb {P}malloc_slow", "nop", "nop"] * (n // 4)), seq=False)
# A value kept in memory across calls (load, add, store through a base register): cheap only if
# the store-to-load hop is renamed away (section 4 of the roofline note).
add("xM", "malloc", ["mov rax, rdi", "lea rcx, [rip + {P}selfp + 8]", "mov rdx, qword ptr [rcx]", "add rdx, 1",
                     "mov qword ptr [rcx], rdx"] , seq=False)
add("xM2", "malloc", ["mov rax, rdi", "lea rcx, [rip + {P}selfp + 8]", "mov rdx, qword ptr [rcx]", "add rdx, 1",
                     "mov qword ptr [rcx], rdx", "mov rdx, qword ptr [rcx]", "add rdx, 1",
                     "mov qword ptr [rcx], rdx"] , seq=False)
# A magazine pop and push without rseq, on a scratch word: (yP, yU); with the CPU number read
# and used as an index of 0 (yPc, yUc); with the arm store (yPa, yUa).
SCR = ["lea r9, [rip + {P}scr]"]
add("yP", "malloc", SCR + ["mov rcx, qword ptr [r9]", "mov rax, qword ptr [rcx - 8]", "lea rcx, [rcx - 8]",
                           "mov qword ptr [r9], rcx"], seq=False)
add("yU", "free", SCR + ["mov rcx, qword ptr [r9]", "lea rax, [rcx + 8]", "mov qword ptr [rcx], rdi",
                         "mov qword ptr [r9], rax"], seq=False)
CPU0 = RS + ["mov r8d, dword ptr fs:[rcx + 4]", "and r8d, 0", "shl r8, 12"]
add("yPc", "malloc", CPU0 + SCR + ["mov rcx, qword ptr [r9 + r8]", "mov rax, qword ptr [rcx - 8]",
                                   "lea rcx, [rcx - 8]", "mov qword ptr [r9 + r8], rcx"], seq=False)
add("yUc", "free", CPU0 + SCR + ["mov rcx, qword ptr [r9 + r8]", "lea rax, [rcx + 8]", "mov qword ptr [rcx], rdi",
                                 "mov qword ptr [r9 + r8], rax"], seq=False)
ARM = RS + ["lea r11, [rip + {P}scr + 64]", "mov qword ptr fs:[rcx + 8], r11", "mov qword ptr fs:[rcx + 8], 0"]
add("yPa", "malloc", ARM + SCR + ["mov rcx, qword ptr [r9]", "mov rax, qword ptr [rcx - 8]", "lea rcx, [rcx - 8]",
                                  "mov qword ptr [r9], rcx"], seq=False)
add("yUa", "free", ARM + SCR + ["mov rcx, qword ptr [r9]", "lea rax, [rcx + 8]", "mov qword ptr [rcx], rdi",
                                "mov qword ptr [r9], rax"], seq=False)
add("xMf", "free", ["lea rcx, [rip + {P}selfp + 8]", "mov rdx, qword ptr [rcx]", "add rdx, 1",
                    "mov qword ptr [rcx], rdx"], seq=False)
add("xM2f", "free", ["lea rcx, [rip + {P}selfp + 8]", "mov rdx, qword ptr [rcx]", "add rdx, 1",
                     "mov qword ptr [rcx], rdx", "mov rdx, qword ptr [rcx]", "add rdx, 1",
                     "mov qword ptr [rcx], rdx"], seq=False)
add("xD", "malloc", ["mov rax, rdi", "mov rcx, qword ptr [rip + {P}selfp]", "mov rcx, qword ptr [rcx]",
                     "mov rcx, qword ptr [rcx]", "mov qword ptr [rip + {P}selfp + 8], rcx"], seq=False)
add("fE", "free", [], seq=False)
for n, m in MALLOC.items():
    if m.struct == "rust":
        add(n, "malloc", malloc_rust("{P}", m), seq=False)
    else:
        body, tail = malloc_fused("{P}", m)
        add(n, "malloc", body, tail)
add("fZ", "free", free_sized("{P}"))
for n, m in (("cD", M(idx="d", short=True)), ("cS", M(short=True)), ("cT", M(idx="t", short=True))):
    body, tail = calloc_fused("{P}", m)
    # After the commit: hand the block and the byte count to the zeroing tail.
    add(n, "calloc", body, tail, epilogue=["mov rsi, rdi", "mov rdi, rax", "jmp {P}calloc_zero"])
for n, f in FREE.items():
    if f.struct == "rust":
        body, tail = free_rust("{P}", f)
        add(n, "free", body, tail, seq=False)
    else:
        body, tail = free_fused("{P}", f)
        add(n, "free", body, tail)


def routine_text(P, name, body, tail, seq):
    sym = f"{P}{name}"
    lines = [l.replace("{P}", P) for l in body]
    text = [".text", ".balign 64", f".globl {sym}", f".type {sym},@function", f"{sym}:", f"{sym}_h:"] + lines
    if seq:
        text += descriptor(P)
    jumps_away = lines and lines[-1].startswith("jmp")
    if name in EPILOGUE:
        text += [l.replace("{P}", P) for l in EPILOGUE[name]]
    elif not jumps_away:
        text += ["9:", "ret"]
    text += [f".globl {sym}_hot_end", f"{sym}_hot_end:"] + [l.replace("{P}", P) for l in tail]
    text += [f".size {sym}, .-{sym}"]
    return "\n".join(text)


def data_text(P, lib, stubs=True):
    vis = ".hidden" if True else ""
    syms = []

    def obj(name, size, align=64, pub=True):
        s = [f".balign {align}", f".globl {P}{name}", f"{vis} {P}{name}", f".type {P}{name},@object",
             f".size {P}{name}, {size}", f"{P}{name}:"]
        if pub:
            # A default-visibility alias at the same address: references to it from a shared
            # library go through the GOT and cannot be relaxed to a lea.
            s = s[:1] + [f".globl {P}{name}_pub", f".type {P}{name}_pub,@object", f".size {P}{name}_pub, {size}",
                         f"{P}{name}_pub:"] + s[1:]
        syms.append(name)
        return s + [f"  .zero {size}"]

    out = [".data", ".balign 64", f"{P}selfp:", f".quad {P}selfp", ".quad 0", ".balign 64",
           f"{P}scr:", f".quad {P}scrmag + 1024", ".balign 64", f"{P}scrmag:", ".zero 2048",
           ".section .bss", ".balign 4096"]
    out += obj("space", 8) + obj("shift", 8, 8) + obj("baseidx", 8, 8) + obj("rseq_off", 8, 8)
    out += obj("regbias", 8, 8) + obj("sizeptr", 129 * 8)
    out += obj("abort_count", 8, 64, pub=False)
    out += obj("masks4", 1024) + obj("size_s_sep", 129) + obj("packed", 129 * 8) + obj("regclass_sep", 4096)
    # The front block: tables at fixed offsets below the slabs.
    out += [".balign 4096", f".globl {P}front", f".hidden {P}front", f"{P}front:", "  .zero 20480"]
    out += obj("slabs", 8192 * 4096, 4096)
    out += [".text", ".balign 64"]
    for slow in ("malloc_slow", "pop_slow", "push_slow", "calloc_slow", "calloc_zero") if stubs else ():
        out += [f".globl {P}{slow}", f".hidden {P}{slow}", f".type {P}{slow},@function", f"{P}{slow}:", "ud2",
                f".size {P}{slow}, .-{P}{slow}"]
    # free(NULL) is legal and common; anything else on the slow path is a broken kernel.
    out += [] if not stubs else [f".globl {P}free_slow", f".hidden {P}free_slow", f".type {P}free_slow,@function", f"{P}free_slow:",
            "test rdi, rdi", "jnz 1f", "ret", "1: ud2", f".size {P}free_slow, .-{P}free_slow"]
    # A getter, so the driver can reach this copy's data without caring how it was linked.
    out += [".balign 64", f".globl {P}data", f".type {P}data,@function", f"{P}data:"]
    out += ["lea rax, [rip + {0}front]".format(P), "mov qword ptr [rdi], rax"]
    for i, s in enumerate(syms):
        out += [f"lea rax, [rip + {P}{s}]", f"mov qword ptr [rdi + {8 * (i + 1)}], rax"]
    out += ["ret", f".size {P}data, .-{P}data"]
    return out, syms


def write_routines(path, P, lib, exports=None):
    """All routines with prefix P. With `exports` ({c_name: routine}), a preloadable stand-in:
    the slow paths come from standin.c and the named routines are also exported as C names."""
    data, syms = data_text(P, lib, stubs=exports is None)
    parts = [".intel_syntax noprefix", "\n".join(data)]
    for name, kind, body, tail, seq in ROUTINES:
        parts.append(routine_text(P, name, body, tail, seq))
    for cname, r in (exports or {}).items():
        # The C name, and a hidden one the library's own C code calls without the PLT.
        parts.append(f".globl {cname}\n.type {cname},@function\n.set {cname}, {P}{r}\n"
                     f".globl {P}{cname}_fast\n.hidden {P}{cname}_fast\n.type {P}{cname}_fast,@function\n"
                     f".set {P}{cname}_fast, {P}{r}")
    parts.append('.section .note.GNU-stack,"",@progbits')
    with open(path, "w") as f:
        f.write("\n\n".join(parts) + "\n")
    return syms


# ---- loop kernels (in the executable) ------------------------------------------------------
# Kernel arguments: rdi iterations, rsi array for bursts, rdx size, rcx the sizes array (64 u32).
# Callee-saved: r13 counter, rbx array, r12 size, r15 sizes, r14 index.

# How a kernel reaches a routine: d a direct call to the copy in the executable; p through the
# PLT; g `call [GOT]` (-fno-plt); r the GOT entry loaded into a register, then `call reg`;
# R (pair loops only) both targets loaded into callee-saved registers before the loop.
CALLS = {
    "d": lambda s, role: [f"call X_{s}"],
    "p": lambda s, role: [f"call L_{s}@PLT"],
    "g": lambda s, role: [f"call qword ptr [rip + L_{s}@GOTPCREL]"],
    "r": lambda s, role: [f"mov rax, qword ptr [rip + L_{s}@GOTPCREL]", "call rax"],
    "R": lambda s, role: [f"call {'rbx' if role == 'm' else 'r15'}"],
}

PAIRS = [
    # (malloc, free) for the C entries; the first is the call floor.
    ("mE", "fE"), ("xA", "fE"), ("xL", "fE"), ("xS", "fE"), ("xD", "fE"), ("xF", "fE"),
    ("xG", "fE"), ("xH", "fE"), ("xI", "fE"), ("xM", "fE"), ("xM2", "fE"),
    ("xM", "xMf"), ("xM2", "xM2f"), ("yP", "yU"), ("yPc", "yUc"), ("yPa", "yUa"),
    *[(f"x{k}{n}", "fE") for k in "NWB" for n in (4, 8, 12, 16, 20)],
    ("mR", "fR"), ("mA", "fA"), ("mA", "fB"), ("mS", "fS"), ("mD", "fS"), ("mP", "fS"), ("mT", "fS"),
    ("mK", "fS"), ("mS", "fSx"), ("mS", "fC"), ("mS", "fW"), ("mS", "fSn"), ("mSg", "fSg"), ("mSo", "fSo"),
    ("mSj", "fSj"), ("mDj", "fSj"), ("mTj", "fWj"), ("mDj", "fWj"), ("mDj", "fWjn"),
    # Mixed: C malloc with the Rust free of a known class, and the reverse, to split a pair's cost.
    ("mS", "push_cls"), ("pop_cls", "fS"), ("mDj", "push_cls"), ("pop_cls", "fWj"),
    # Rust: generic routines with the class in edi, and per-class routines.
    ("pop_gen", "push_gen"), ("pop_cls", "push_cls"),
    # Rust's dealloc with a run-time size: the class from the size, not the address.
    ("mS", "fZ"), ("mR", "fZ"),
    # Shorter address chains for the free.
    ("mSb", "fSb"), ("mAb", "fBb"), ("mSb", "fS"), ("mS", "fSb"), ("mA", "fS"), ("mS", "fB"),
    ("mS", "fB1"), ("mS", "fB2"), ("mS", "fB3"), ("mS", "fS1"), ("mS", "fS2"), ("mS", "fS3"),
    ("mS", "fU"), ("mS", "fUj"), ("mS", "fUw"), ("mS", "fV"), ("mS", "fVj"), ("mS", "fVw"),
    ("mLj", "fWjn"), ("mSj", "fUj"), ("mSj", "fVj"), ("mDj", "fUj"), ("mTj", "fUj"), ("mSo", "fUj"), ("mLj", "fUj"), ("mLj", "fUjn"), ("mL", "fU"), ("mQj", "fUj"), ("mQj", "fWjn"), ("mQ", "fU"), ("mDj", "fVj"), ("mR", "fU"), ("mR", "fV"),
    ("mY", "fY"), ("mY", "fYb"), ("mYj", "fYj"), ("mYj", "fYbj"), ("mYdj", "fYbj"), ("mYtj", "fYbj"),
    ("mYj", "fYwj"), ("mYj", "fYwbj"), ("mR", "fY"), ("mR", "fYb"),
]
BURSTS = (8, 32)


def malloc_call(m, call):
    if m == "pop_gen":
        return [f"mov edi, {CLASS}"] + CALLS[call](m, "m")
    return CALLS[call](m, "m")


def free_call(f, call, src, size="r12d"):
    if f == "fZ":
        return [f"mov rdi, {src}", f"mov esi, {size}"] + CALLS[call](f, "f")
    if f == "push_gen":
        return [f"mov rsi, {src}", f"mov edi, {CLASS}"] + CALLS[call](f, "f")
    return [f"mov rdi, {src}"] + CALLS[call](f, "f")


kernels = []  # (name, form, malloc, free, call, ops per iteration)
kasm = []


def kernel(name, body, ops, meta):
    head = [".text", ".balign 64", f".globl {name}", f".type {name},@function", f"{name}:", "push rbx", "push r12",
            "push r13", "push r14", "push r15", "mov r13, rdi", "mov rbx, rsi", "mov r12, rdx", "mov r15, rcx",
            "xor r14d, r14d"]
    tail = ["pop r15", "pop r14", "pop r13", "pop r12", "pop rbx", "ret", f".size {name}, .-{name}"]
    kasm.append("\n".join(head + body + tail))
    kernels.append((name, ops) + meta)


for m, f in PAIRS:
    for call in CALLS:
        tag = f"{m}_{f}_{call}"
        if m.startswith("pop") or m == "mE":
            setsize = []
        else:
            setsize = ["mov edi, r12d"]
        pre = ([f"mov rbx, qword ptr [rip + L_{m}@GOTPCREL]", f"mov r15, qword ptr [rip + L_{f}@GOTPCREL]"]
               if call == "R" else [])
        kernel(f"kp_{tag}", pre + [".balign 64", "1:"] + setsize + malloc_call(m, call) + free_call(f, call, "rax")
               + ["dec r13", "jnz 1b"], 1, ("pair", m, f, call))
        if call == "R":
            continue
        if not m.startswith("pop") and m != "mE" and not f.endswith(("_cls", "_gen")):
            kernel(f"km_{tag}", [".balign 64", "1:", "mov edi, dword ptr [r15 + r14*4]"] + malloc_call(m, call)
                   + free_call(f, call, "rax", "dword ptr [r15 + r14*4]") + ["inc r14d", "and r14d, 63", "dec r13", "jnz 1b"], 1,
                   ("mixed", m, f, call))
        for k in BURSTS:
            body = [".balign 64", "1:", "xor r14d, r14d", ".balign 32", "2:"] + setsize + malloc_call(m, call) + [
                "mov qword ptr [rbx + r14*8], rax", "inc r14", f"cmp r14, {k}", "jne 2b", "xor r14d, r14d",
                ".balign 32", "3:"] + free_call(f, call, "qword ptr [rbx + r14*8]") + [
                "inc r14", f"cmp r14, {k}", "jne 3b", "dec r13", "jnz 1b"]
            kernel(f"kb{k}_{tag}", body, k, (f"burst{k}", m, f, call))


# free(NULL) in a loop: the null pointer takes the free's slow path unless the routine tests it.
for f in ("fUj", "fWjn", "fSn", "fS", "fR"):
    for call in ("d", "p"):
        kernel(f"kn_{f}_{call}", [".balign 64", "1:", "xor edi, edi"] + CALLS[call](f, "f") + ["dec r13", "jnz 1b"],
               1, ("null", "", f, call))


# Load-to-use latency by addressing mode: a chase through zeroed memory, so the loaded value
# (0) is the next index. rbx points into the middle of a zeroed buffer.
LAT = {
    "q_b": "mov rax, qword ptr [rbx + rax]",
    "q_bd8": "mov rax, qword ptr [rbx + rax + 8]",
    "q_bd32": "mov rax, qword ptr [rbx + rax - 12288]",
    "q_bs8": "mov rax, qword ptr [rbx + rax*8]",
    "q_bs8d32": "mov rax, qword ptr [rbx + rax*8 - 12288]",
    "q_r": "mov rax, qword ptr [rax + 0]",
    "q_rd8": "mov rax, qword ptr [rax + 8]",
    "q_rd32": "mov rax, qword ptr [rax + 4096]",
    "b_b": "movzx eax, byte ptr [rbx + rax]",
    "b_bd32": "movzx eax, byte ptr [rbx + rax - 12288]",
    "b_bd8": "movzx eax, byte ptr [rbx + rax + 8]",
    "d_b": "mov eax, dword ptr [rbx + rax]",
}
for name, ins in LAT.items():
    if "[rax +" in ins:
        disp = ins.split("+")[-1].strip(" ]")
        pre = ["lea rax, [rip + lat_buf + 1024]", f"mov qword ptr [rax + {disp}], rax"]
    else:
        pre = ["lea rbx, [rip + lat_buf + 32768]", "xor eax, eax"]
    kernel(f"kl_{name}", pre + [".balign 64", "1:", ins, "dec r13", "jnz 1b"], 1, ("lat", name, "", ""))
kasm.append(".section .bss\n.balign 4096\nlat_buf:\n.zero 65536\n.text")


# best: the recommended malloc, free and calloc; two: tcmalloc's two-range size index; rust: the
# Rust-structured entries over the generic routines.
STANDINS = {"best": ("mLj", "fUjn", "cD"), "bestnc": ("mLj", "fUjn"), "two": ("mTj", "fUj"), "rust": ("mR", "fR")}


def main():
    out = sys.argv[1]
    os.makedirs(out, exist_ok=True)
    syms = write_routines(f"{out}/routines_lib.S", "L_", True)
    write_routines(f"{out}/routines_exe.S", "X_", False)
    # The stand-in allocators: the shapes worth comparing with the incumbents through the PLT.
    for tag, names in STANDINS.items():
        write_routines(f"{out}/standin_{tag}.S", "S_", True, dict(zip(("malloc", "free", "calloc"), names)))
    with open(f"{out}/kernels.S", "w") as fh:
        fh.write(".intel_syntax noprefix\n\n" + "\n\n".join(kasm) + '\n.section .note.GNU-stack,"",@progbits\n')
    with open(f"{out}/front.h", "w") as fh:
        w = fh.write
        w("/* @generated by gen.py: the data every copy of the routines reads. */\n")
        w("#ifndef CENTRY_FRONT_H\n#define CENTRY_FRONT_H\n#include <stdint.h>\n")
        w("struct front_data {\n  char *front;\n")
        for s_ in syms:
            w(f"  char *{s_};\n")
        w("};\n")
        w(f"enum {{ OFF_REGION = {OFF_REGION}, OFF_MASK = {OFF_MASK}, OFF_SIZE_S = {OFF_SIZE_S}, "
          f"OFF_SIZE_T = {OFF_SIZE_T}, OFF_SIZE_D = {OFF_SIZE_D}, OFF_SIZE_L = {OFF_SIZE_L}, RUST_CLASS = {CLASS}, SMALL = {SMALL} }};\n")
        w("#endif\n")
    with open(f"{out}/table.h", "w") as fh:
        w = fh.write
        w("/* @generated by gen.py */\n#include <stdint.h>\n#include \"front.h\"\n")
        w("typedef uint64_t (*kern_fn)(uint64_t, uint64_t *, uint64_t, const uint32_t *);\n")
        w("void L_data(struct front_data *);\nvoid X_data(struct front_data *);\n")
        for name, *_ in kernels:
            w(f"uint64_t {name}(uint64_t, uint64_t *, uint64_t, const uint32_t *);\n")
        w("struct kernel { const char *name; kern_fn f; uint64_t ops; const char *form, *m, *f_, *call; };\n")
        w("static const struct kernel KERNELS[] = {\n")
        for name, ops, form, m, f, call in kernels:
            w(f'  {{"{name}", {name}, {ops}, "{form}", "{m}", "{f}", "{call}"}},\n')
        w("};\n")
        for P in ("L_", "X_"):
            for name, kind, body, tail, seq in ROUTINES:
                w(f"void {P}{name}(void); extern const char {P}{name}_hot_end[];\n")
        w("struct routine { const char *name, *kind; const void *entry, *end; int uops; };\n")
        for P in ("L_", "X_"):
            w(f"static const struct routine ROUTINES_{P}[] = {{\n")
            for name, kind, body, tail, seq in ROUTINES:
                text = body + (descriptor("") if seq else []) + (
                    EPILOGUE.get(name) or ([] if body and body[-1].startswith("jmp") else ["ret"]))
                u = fused_uops(hot_path(text))
                w(f'  {{"{name}", "{kind}", (const void *){P}{name}, {P}{name}_hot_end, {u}}},\n')
            w("};\n")


main()
