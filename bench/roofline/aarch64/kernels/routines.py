"""Candidates for cralloc's own routines, varying one thing at a time.

A baseline pop and push follow the design's shape in the `executable` form, with the word's address
in x3 and the mask in w2 as inlined Rust hands them over, and each option below changes one step.
Options, as keyword arguments of `pop` and `push`:

  rseq    how the rseq area's address is found: exe (adrp+ldr of a copy), got (adrp+ldr of the GOT
          slot, then a load), word (the same through a word of the routine's own), hoist (x26)
  cpu     the CPU check: cmp (cmp w, #8192; b.hs), tbnz (bit 31 only), ubfiz (no check: the top
          slabs are never written), adds (x8 = word + cpu << 12 by add lsl after cmp)
  addr    the word's address: reg ([x3, x8]) or add (add x9, x3, x8; [x9]) or addlsl (add x9, x3,
          x8, lsl #12, no separate shift)
  order   the slot's load against the new word: load_first, sub_first, pre (pre-index)
  mask    reg (tst w, w2) or imm (tst w, #0x1f0)
  arm     adrp (adrp+add of the descriptor) or got (adrp+ldr, as tcmalloc), reuse (the area's
          address kept in x10 from an earlier step of the same pair)
"""
from floors import RSEQ_SIG, batch, calls, pair, rseq_section

MASK_IMM = "#0x1f0"


def area(rseq):
    if rseq == "exe":
        return """  adrp x9, __rseq_offset
  ldr x9, [x9, :lo12:__rseq_offset]
  mrs x10, tpidr_el0
  add x10, x10, x9"""
    if rseq == "got":
        return """  adrp x9, :got:__rseq_offset
  ldr x9, [x9, :got_lo12:__rseq_offset]
  ldr x9, [x9]
  mrs x10, tpidr_el0
  add x10, x10, x9"""
    if rseq == "word":
        return """  adrp x9, rf_rseq_word
  ldr x9, [x9, :lo12:rf_rseq_word]
  ldr x9, [x9]
  mrs x10, tpidr_el0
  add x10, x10, x9"""
    if rseq == "hoist":
        return "  mov x10, x26"
    if rseq == "none":
        return ""
    raise ValueError(rseq)


def arming(p, arm):
    """The descriptor, then the arming store alone, where an abort returns. A sequence holds no
    relocation: a linker working around Cortex-A53 erratum 843419 can replace a load after an `adrp`
    with a branch to a stub, and a preemption in the stub would not abort it."""
    if arm == "adrp":
        return f"""  adrp x11, {p}_cs
  add x11, x11, :lo12:{p}_cs
{p}_restart:
  str x11, [x10, #8]"""
    if arm == "got":
        return f"""  adrp x11, :got:{p}_cs
  ldr x11, [x11, :got_lo12:{p}_cs]
{p}_restart:
  str x11, [x10, #8]"""
    raise ValueError(arm)


def cpu_step(cpu):
    """Leaves the CPU number, shifted as `addr` wants it, in x8 and returns text."""
    if cpu == "cmp":
        return """  ldr w8, [x10, #4]
  cmp w8, #2, lsl #12
  b.hs 9f
  lsl x8, x8, #12"""
    if cpu == "cmps":
        # The compare's own flags: an unshifted number for addr=addlsl.
        return """  ldr w8, [x10, #4]
  cmp w8, #2, lsl #12
  b.hs 9f"""
    if cpu == "tbnz":
        return """  ldr w8, [x10, #4]
  tbnz w8, #31, 9f
  lsl x8, x8, #12"""
    if cpu == "ubfiz":
        return """  ldr w8, [x10, #4]
  ubfiz x8, x8, #12, #13"""
    if cpu == "nocheck":
        return """  ldr w8, [x10, #4]
  lsl x8, x8, #12"""
    raise ValueError(cpu)


def word_address(addr):
    """Returns (text before the load, the memory operand)."""
    if addr == "reg":
        return "", "[x3, x8]"
    if addr == "add":
        return "  add x9, x3, x8", "[x9]"
    if addr == "addlsl":
        return "  add x9, x3, x8, lsl #12", "[x9]"
    raise ValueError(addr)


def pop(p, rseq="exe", cpu="cmp", addr="reg", order="load_first", mask="reg", arm="adrp", inputs="", area_after=False):
    pre, m = word_address(addr)
    tst = f"  tst w1, {MASK_IMM}" if mask == "imm" else "  tst w1, w2"
    if order == "load_first":
        take = f"""  ldr x0, [x1, #-8]
  sub x1, x1, #8"""
    elif order == "sub_first":
        take = f"""  sub x1, x1, #8
  ldr x0, [x1]"""
    elif order == "pre":
        take = "  ldr x0, [x1, #-8]!"
    else:
        raise ValueError(order)
    return f"""{inputs}
{area(rseq)}
{arming(p, arm)}
{p}_start:
{cpu_step(cpu)}
{pre}
  ldr x1, {m}
{tst}
  b.eq 9f
{take}
  str x1, {m}
{p}_end:
{rseq_section(None, "pop", p)}"""


def push(p, rseq="exe", cpu="cmp", addr="reg", order="add_first", mask="reg", arm="adrp", inputs=""):
    pre, m = word_address(addr)
    if order == "add_first":
        check = f"""  add x4, x1, #8
  tst w4, {'w2' if mask == 'reg' else MASK_IMM}
  b.eq 9f
  str x0, [x1]
  str x4, {m}"""
    elif order == "post":
        # Tests the word as it is and stores through post-index: the test must look one on, which
        # a mask of the entry-number bits of word + 8 needs.
        check = f"""  add x4, x1, #8
  tst w4, {'w2' if mask == 'reg' else MASK_IMM}
  b.eq 9f
  str x0, [x1], #8
  str x1, {m}"""
    else:
        raise ValueError(order)
    return f"""{inputs}
{area(rseq)}
{arming(p, arm)}
{p}_start:
{cpu_step(cpu)}
{pre}
  ldr x1, {m}
{check}
{p}_end:
{rseq_section(None, "push", p)}"""


# The inputs as they stand in the loop's registers, or formed at each call.
HOIST_INPUTS = """  mov x25, x20
  mrs x8, tpidr_el0
  add x26, x8, x21
  ldr w8, [x26, #4]
  sub x27, x20, x8, lsl #12
  add x3, x27, x23
  mov w2, #0x1f0
"""
FORM_WORD = """  adrp x3, rf_slabs + 152
  add x3, x3, :lo12:rf_slabs + 152
  mov w2, #0x1f0"""
FORM_WORD_ONLY = """  adrp x3, rf_slabs + 152
  add x3, x3, :lo12:rf_slabs + 152"""


def candidate(k, tag, note, pop_args=None, push_args=None, which="pcb", setup=HOIST_INPUTS):
    pop_args = pop_args or {}
    push_args = push_args or {}
    if "p" in which:
        pair(k, f"i_{tag}", pop(f"rf_i_{tag}_a", **pop_args), push(f"rf_i_{tag}_f", **push_args), setup=setup, note=note)
    if "c" in which:
        calls(k, f"i_{tag}", pop(f"rf_ic_{tag}_a", **pop_args), push(f"rf_ic_{tag}_f", **push_args), setup=setup, note=note)
    if "b" in which:
        batch(k, f"i_{tag}", pop(f"rf_ib_{tag}_a", **pop_args), push(f"rf_ib_{tag}_f", **push_args), setup=setup, note=note)


def stale_pop(p, reg, rseq="exe", arm="adrp", hoist_mask=True):
    """The word's address is compared with what a scratch register still holds from the last call,
    and loaded through that register, which is not written: the core then renames the load."""
    return f"""{area(rseq)}
{arming(p, arm)}
{p}_start:
  ldr w8, [x10, #4]
  cmp w8, #2, lsl #12
  b.hs 9f
  add x9, x3, x8, lsl #12
  cmp {reg}, x9
  b.ne {p}_fix
  ldr x1, [{reg}]
  tst w1, w2
  b.eq 9f
  ldr x0, [x1, #-8]
  sub x1, x1, #8
  str x1, [{reg}]
{p}_end:
{rseq_section(None, "pop", p)}
  .subsection 1
{p}_fix:
  mov {reg}, x9
  b {p}_restart
  .subsection 0"""


def stale_push(p, reg, rseq="exe", arm="adrp"):
    return f"""{area(rseq)}
{arming(p, arm)}
{p}_start:
  ldr w8, [x10, #4]
  cmp w8, #2, lsl #12
  b.hs 9f
  add x9, x3, x8, lsl #12
  cmp {reg}, x9
  b.ne {p}_fix
  ldr x1, [{reg}]
  add x4, x1, #8
  tst w4, w2
  b.eq 9f
  str x0, [x1]
  str x4, [{reg}]
{p}_end:
{rseq_section(None, "push", p)}
  .subsection 1
{p}_fix:
  mov {reg}, x9
  b {p}_restart
  .subsection 0"""


def base_cpu(check, reg="x9"):
    """x9 = the slab of this CPU, from x10 the rseq area and x13 the slabs, found before the
    sequence. `ubfiz` needs no branch: any number lands on a slab, the top two (which negative
    numbers reach) never written; `cmp` branches instead."""
    if check == "ubfiz":
        return f"""  ldr w8, [x10, #4]
  ubfiz x8, x8, #12, #13
  add {reg}, x13, x8"""
    if check == "ubfiz_nosym":
        return f"""  ldr w8, [x10, #4]
  ubfiz x8, x8, #12, #13
  add {reg}, x27, x8"""
    if check == "cmp":
        return f"""  ldr w8, [x10, #4]
  cmp w8, #2, lsl #12
  b.hs 9f
  add {reg}, x13, x8, lsl #12"""
    if check == "cmp_nosym":
        return f"""  ldr w8, [x10, #4]
  cmp w8, #2, lsl #12
  b.hs 9f
  add {reg}, x27, x8, lsl #12"""
    raise ValueError(check)


def base_operand(word):
    return f"#{word}" if isinstance(word, int) else word


def s2_pop(p, reg="x18", check="ubfiz", rseq="exe", word=152, mask="#0x1f0", arm="adrp"):
    w = base_operand(word)
    return f"""  adrp x13, rf_slabs
{area(rseq)}
{arming(p, arm)}
{p}_start:
{base_cpu(check)}
  cmp {reg}, x9
  b.ne {p}_fix
  ldr x1, [{reg}, {w}]
  tst w1, {mask}
  b.eq 9f
  ldr x0, [x1, #-8]
  sub x1, x1, #8
  str x1, [{reg}, {w}]
{p}_end:
{rseq_section(None, "pop", p)}
  .subsection 1
{p}_fix:
  mov {reg}, x9
  b {p}_restart
  .subsection 0"""


def s2_push(p, reg="x18", check="ubfiz", rseq="exe", word=152, mask="#0x1f0", arm="adrp"):
    w = base_operand(word)
    return f"""  adrp x13, rf_slabs
{area(rseq)}
{arming(p, arm)}
{p}_start:
{base_cpu(check)}
  cmp {reg}, x9
  b.ne {p}_fix
  ldr x1, [{reg}, {w}]
  add x4, x1, #8
  tst w4, {mask}
  b.eq 9f
  str x0, [x1]
  str x4, [{reg}, {w}]
{p}_end:
{rseq_section(None, "push", p)}
  .subsection 1
{p}_fix:
  mov {reg}, x9
  b {p}_restart
  .subsection 0"""


def s2(k, tag, note, **args):
    pair(k, f"s2_{tag}", s2_pop(f"rf_s2_{tag}_a", **args), s2_push(f"rf_s2_{tag}_f", **args), setup=HOIST_INPUTS, note=note)
    calls(k, f"s2_{tag}", s2_pop(f"rf_s2c_{tag}_a", **args), s2_push(f"rf_s2c_{tag}_f", **args), setup=HOIST_INPUTS, note=note)
    batch(k, f"s2_{tag}", s2_pop(f"rf_s2b_{tag}_a", **args), s2_push(f"rf_s2b_{tag}_f", **args), setup=HOIST_INPUTS, note=note)


def stale_base(k):
    s2(k, "x18", "slab base stale in x18, ubfiz, exe")
    s2(k, "x17", "slab base stale in x17, ubfiz, exe", reg="x17")
    s2(k, "x15", "slab base stale in x15, ubfiz, exe", reg="x15")
    s2(k, "got", "slab base stale in x18, ubfiz, got", rseq="got")
    s2(k, "cmp", "slab base stale in x18, cmp, exe", check="cmp")
    s2(k, "hoist", "slab base stale, area hoisted", rseq="hoist")
    s2(k, "nosym", "slab base stale, slabs' address hoisted, ubfiz", check="ubfiz_nosym")
    s2(k, "hoist2", "slab base stale, slabs' address and area hoisted, ubfiz", check="ubfiz_nosym", rseq="hoist")
    s2(k, "rt", "slab base stale, class in a register: [x18, x23]: no renaming", word="x23", mask="w2")
    s2(k, "rtcmp", "slab base stale, class in a register, cmp", word="x23", mask="w2", check="cmp")
    # What a clobbered register costs: the fix path every time, and on a random one in N.
    k.kernel("p_s2_clob", "  mov x18, #0\n" + s2_pop("rf_s2_clob_a") + "\n  mov x18, #0\n" + s2_push("rf_s2_clob_f"), setup=HOIST_INPUTS, kind="Slab", note="pair: x18 clobbered before each call: the fix path, predicted")
    streams = {2: "half", 3: "a quarter", 4: "an eighth", 5: "a sixteenth"}
    for st, what in streams.items():
        setup = HOIST_INPUTS + f"  mov x25, #({st + 1} << 18)\n  add x25, x22, x25\n  mov x24, #0\n"
        # A clobber when the stream's class is even: a random pattern with the stream's density.
        clob = """  ldrh w5, [x25, x24, lsl #1]
  add x24, x24, #1
  and x24, x24, #0xffff
  and w5, w5, #32
  cbnz w5, 3f
  mov x18, #0
3:
"""
        if st in (2,):
            k.kernel(f"p_s2_cl{st}", clob + s2_pop(f"rf_s2_cl{st}_a") + "\n" + s2_push(f"rf_s2_cl{st}_f"), setup=setup, kind="Slab", note=f"pair: x18 clobbered on a random half of pairs")

    # Mixed classes in a register, word through [x18, x5].
    for st, what in {0: "one class", 2: "random of 2", 3: "random of 4", 4: "random of 8", 5: "random of 16", 6: "same as last 7 in 8", 7: "cycle of 8"}.items():
        setup = HOIST_INPUTS + f"  mov x25, #({st + 1} << 18)\n  add x25, x22, x25\n  mov x24, #0\n"
        next_class = """  ldrh w5, [x25, x24, lsl #1]
  add x24, x24, #1
  and x24, x24, #0xffff
"""
        args = dict(word="x5", mask="w2")
        k.kernel(f"m_s2_{st}", next_class + s2_pop(f"rf_m_s2{st}_a", **args) + "\n" + s2_push(f"rf_m_s2{st}_f", **args), setup=setup, kind="Slab", note=f"slab base stale: pair of a class from: {what}")
        batch_body = f"""  mov x28, #0
  .balign 32
3:
{next_class}{s2_pop(f"rf_mb_s2{st}_a", **args)}
  str x0, [x22, x28, lsl #3]
  add x28, x28, #1
  cmp x28, #32
  b.ne 3b
  sub x24, x24, #32
  and x24, x24, #0xffff
  mov x28, #0
  .balign 32
4:
{next_class}  ldr x0, [x22, x28, lsl #3]
{s2_push(f"rf_mb_s2{st}_f", **args)}
  add x28, x28, #1
  cmp x28, #32
  b.ne 4b"""
        k.kernel(f"mb_s2_{st}", batch_body, setup=setup, kind="Slab", ops=32, note=f"slab base stale: batch 32 of classes from: {what}")
    # Two constant classes alternating: renamed each.
    k.kernel("m_s2c_2", s2_pop("rf_m_s2c_a") + "\n" + s2_push("rf_m_s2c_b") + "\n" + s2_pop("rf_m_s2c_c", word=184) + "\n" + s2_push("rf_m_s2c_d", word=184), setup=HOIST_INPUTS, kind="Slab", ops=2, note="slab base stale, 2 constant classes alternating, per pair")
    k.kernel("m_base_c2", pop("rf_m_bc_a") + "\n" + push("rf_m_bc_b") + "\n  add x3, x3, #32\n" + pop("rf_m_bc_c") + "\n" + push("rf_m_bc_d") + "\n  sub x3, x3, #32", setup=HOIST_INPUTS, kind="Slab", ops=2, note="base, 2 constant classes alternating, per pair")


def nops(n):
    return "\n".join("  nop" for _ in range(n))


def placement(k):
    """The stale-base pair and the plain one, with their position in the rename groups moved: `a`
    nops before the pop, `b` between pop and push, `c` extra before the loop's start."""
    for a in range(0, 8):
        for b in range(0, 8):
            for tag, args in (("x", {}), ("h", {"check": "ubfiz_nosym", "rseq": "hoist"})):
                k.kernel(f"ps_{tag}_{a}_{b}", f"{nops(a)}\n{s2_pop(f'rf_ps_{tag}{a}{b}_a', **args)}\n{nops(b)}\n{s2_push(f'rf_ps_{tag}{a}{b}_f', **args)}",
                         setup=HOIST_INPUTS, kind="Slab", note=f"stale-base pair, {a} nops before, {b} nops between")
    for c in range(0, 32):
        for tag, args in (("x", {}), ("h", {"check": "ubfiz_nosym", "rseq": "hoist"})):
            k.kernel(f"pc_{tag}_{c}", f"{s2_pop(f'rf_pc_{tag}{c}_a', **args)}\n{s2_push(f'rf_pc_{tag}{c}_f', **args)}",
                     setup=HOIST_INPUTS, kind="Slab", pad=c, align=128, note=f"stale-base pair, loop at byte {c * 4} of 128")
    # A caller's work between the calls: does the hop stay free when more of the program runs?
    for n in (1, 2, 4, 8, 16, 32, 64):
        work = "\n".join(f"  add x{5 + i % 3}, x{5 + i % 3}, #1" for i in range(n))
        k.kernel(f"pw_x_{n}", f"{s2_pop(f'rf_pw_x{n}_a')}\n{work}\n{s2_push(f'rf_pw_x{n}_f')}\n{work}",
                 setup=HOIST_INPUTS, kind="Slab", note=f"stale-base pair, {n} adds after each call")
        k.kernel(f"pw_b_{n}", f"{pop(f'rf_pw_b{n}_a')}\n{work}\n{push(f'rf_pw_b{n}_f')}\n{work}",
                 setup=HOIST_INPUTS, kind="Slab", note=f"plain pair, {n} adds after each call")
        k.kernel(f"pw_n_{n}", f"{work}\n{work}", setup=HOIST_INPUTS, kind="Slab", note=f"no pair: the {2 * n} adds alone")
    # A caller bound by the front end, not the chain: what inline and out of line add to its work.
    # The pops and pushes are the plain shape; each pass has 2n nops of the caller's.
    def fill(n):
        return nops(n)
    call_fns = f""".balign 64
rf_pwn_pop:
{pop("rf_pwn_a")}
  ret
9:
  brk #1
.balign 64
rf_pwn_push:
{push("rf_pwn_f")}
  ret
9:
  brk #1
"""
    k.raw(".text\n" + call_fns)
    for n in (32, 64, 128, 256):
        k.kernel(f"pwn_i_{n}", f"{pop(f'rf_pwn_i{n}_a')}\n{fill(n)}\n{push(f'rf_pwn_i{n}_f')}\n{fill(n)}", setup=HOIST_INPUTS, kind="Slab", note=f"plain pair inline, {2 * n} nops a pass")
        k.kernel(f"pwn_c_{n}", f"  bl rf_pwn_pop\n{fill(n)}\n  bl rf_pwn_push\n{fill(n)}", setup=HOIST_INPUTS, kind="Slab", note=f"plain pair through 2 calls, {2 * n} nops a pass")
        k.kernel(f"pwn_n_{n}", f"{fill(n)}\n{fill(n)}", setup=HOIST_INPUTS, kind="Slab", note=f"{2 * n} nops a pass")
    # A lone call: how long from the start of a pop until its pointer can be used.
    use = "\n".join("  add x0, x0, #1" for _ in range(8))
    for tag, p_args in (("plain", None), ("got", {"rseq": "got"})):
        k.kernel(f"lone_{tag}", f"  isb\n{pop(f'rf_lone_{tag}', **(p_args or {}))}\n{use}\n  isb\n  sub x0, x0, #8\n{push(f'rf_lone_{tag}_f', **(p_args or {}))}",
                 setup=HOIST_INPUTS, kind="Slab", note=f"isb; pop {tag}; 8 adds on the pointer; isb; push")
    for tag, p_args in (("sb", {}), ("sbgot", {"rseq": "got"})):
        k.kernel(f"lone_{tag}", f"  isb\n{s2_pop(f'rf_lone_{tag}', **p_args)}\n{use}\n  isb\n  sub x0, x0, #8\n{s2_push(f'rf_lone_{tag}_f', **p_args)}",
                 setup=HOIST_INPUTS, kind="Slab", note=f"isb; pop stale base {tag}; 8 adds on the pointer; isb; push")
    k.kernel("lone_none", f"  isb\n  mov x0, x20\n{use}\n  isb", setup=HOIST_INPUTS, kind="Slab", note="isb; a register; 8 adds; isb: the frame")


def lean_prologue(p, rseq="word", setup="", late=False):
    """The rseq area in x10 and the routine armed, the descriptor's address in x11 so that a restart
    is only the arming store. `word`, the default form: one `adrp`, since the routine's own word
    for __rseq_offset's address follows its descriptor, and `adrp` competes with branches. `exe`:
    a direct load, which needs a copy relocation. `got`: through the GOT. `setup` forms what the
    sequence reads, since the sequence itself holds no relocation: ahead of the area, or after it
    if `late`."""
    desc = f"""  adrp x11, {p}_cs
  add x11, x11, :lo12:{p}_cs"""
    if rseq == "word":
        offset = """  ldr x9, [x11, #32]
  ldr x9, [x9]"""
    elif rseq == "exe":
        offset = """  adrp x9, __rseq_offset
  ldr x9, [x9, :lo12:__rseq_offset]"""
    elif rseq == "got":
        offset = """  adrp x9, :got:__rseq_offset
  ldr x9, [x9, :got_lo12:__rseq_offset]
  ldr x9, [x9]"""
    else:
        raise ValueError(rseq)
    first, last = ("", f"\n{setup}") if late else (f"{setup}\n" if setup else "", "")
    return f"""{first}{desc}
{offset}
  mrs x10, tpidr_el0
  add x10, x10, x9{last}
{p}_restart:
  str x11, [x10, #8]
{p}_start:"""


def lean_group(p):
    """The descriptor, the word 32 bytes on, and the abort handler behind its signature."""
    return f"""  .pushsection __rseq_cs, "aw"
  .balign 32
{p}_cs:
  .long 0, 0
  .quad {p}_start, {p}_end - {p}_start, {p}_abort
  .quad __rseq_offset
  .popsection
  .subsection 1
  {RSEQ_SIG}
{p}_abort:
  adrp x16, rf_abort_count
  ldr x17, [x16, :lo12:rf_abort_count]
  add x17, x17, #1
  str x17, [x16, :lo12:rf_abort_count]
  b {p}_restart
  .subsection 0"""


def recommended_pop(p, rseq="word"):
    """The recommended inlined pop: word's address in x3, mask in w2, result in x0. No stub: a miss
    leaves the sequence with the result still zero, so nothing depends on where the build puts
    code, and the abort handler holds only an unconditional branch."""
    return f"""{lean_prologue(p, rseq)}
  mov x0, #0
  ldr w8, [x10, #4]
  cmp w8, #2, lsl #12
  b.hs {p}_end
  lsl x8, x8, #12
  ldr x1, [x3, x8]
  tst w1, w2
  b.eq {p}_end
  ldr x0, [x1, #-8]
  sub x1, x1, #8
  str x1, [x3, x8]
{p}_end:
{lean_group(p)}"""


def recommended_push(p, rseq="word"):
    """The recommended inlined push: pointer in x0, word's address in x3, mask in w2, x4 nonzero
    if pushed."""
    return f"""{lean_prologue(p, rseq)}
  mov x4, #0
  ldr w8, [x10, #4]
  cmp w8, #2, lsl #12
  b.hs {p}_end
  lsl x8, x8, #12
  ldr x1, [x3, x8]
  add x9, x1, #8
  tst w9, w2
  b.eq {p}_end
  str x0, [x1]
  mov x4, x9
  str x9, [x3, x8]
{p}_end:
{lean_group(p)}"""


def recommended(k):
    """The routines as the report recommends them, in each form, with the caller's tests."""
    pop_test = "  cbz x0, 9f"
    push_test = "  cbz x4, 9f"
    for tag, rseq in (("word", "word"), ("got", "got"), ("exe", "exe")):
        def full_pop(p):
            return f"{recommended_pop(p, rseq)}\n{pop_test}"

        def full_push(p):
            return f"{recommended_push(p, rseq)}\n{push_test}"
        pair(k, f"fin_{tag}", full_pop(f"rf_fin_{tag}_a"), full_push(f"rf_fin_{tag}_f"), setup=HOIST_INPUTS, note=f"recommended inline pop and push, {tag}, a constant class hoisted")
        calls(k, f"fin_{tag}", full_pop(f"rf_finc_{tag}_a"), full_push(f"rf_finc_{tag}_f"), setup=HOIST_INPUTS, note=f"recommended pop and push out of line, {tag}")
        batch(k, f"fin_{tag}", full_pop(f"rf_finb_{tag}_a"), full_push(f"rf_finb_{tag}_f"), setup=HOIST_INPUTS, note=f"recommended pop and push inline, {tag}")
        # A class formed at the site: adrp and add for the word, a move for the mask.
        formed = FORM_WORD + "\n"
        pair(k, f"finf_{tag}", formed + full_pop(f"rf_finf_{tag}_a"), formed + full_push(f"rf_finf_{tag}_f"), setup=HOIST_INPUTS, note=f"recommended inline, {tag}, the word and mask formed at each site")
    # The size known at run time: a class byte and a mask from the tables, 64 sizes in a cycle.
    lookup_setup = HOIST_INPUTS + """  mov x25, #(9 << 18)
  add x25, x22, x25
  mov x24, #0
  adrp x26, rf_tables + 2048
  add x26, x26, :lo12:rf_tables + 2048
  adrp x28, rf_tables + 16384 + 2048
  add x28, x28, :lo12:rf_tables + 16384 + 2048
"""
    lookup = """  ldrh w5, [x25, x24, lsl #1]
  add x24, x24, #1
  and x24, x24, #63
  sub x5, x5, #1
  lsr x5, x5, #3
  add x5, x5, #1
  ldrb w5, [x26, x5]
  add x3, x27, x5, lsl #3
  ldr x2, [x28, x5, lsl #3]
"""
    for tag, rseq in (("word", "word"), ("exe", "exe")):
        k.kernel(f"finr_{tag}", f"{lookup}{recommended_pop(f'rf_finr_{tag}_a', rseq)}\n{pop_test}\n{recommended_push(f'rf_finr_{tag}_f', rseq)}\n{push_test}",
                 setup=lookup_setup, kind="Slab", note=f"recommended inline, {tag}, sizes from a table: 64 in a cycle")
    size32 = lookup_setup.replace("#(9 << 18)", "#(9 << 18)")
    lookup32 = """  mov x5, #5
  ldrb w5, [x26, x5]
  add x3, x27, x5, lsl #3
  ldr x2, [x28, x5, lsl #3]
"""
    for tag, rseq in (("word", "word"), ("exe", "exe")):
        k.kernel(f"finr32_{tag}", f"{lookup32}{recommended_pop(f'rf_finr32_{tag}_a', rseq)}\n{pop_test}\n{lookup32}{recommended_push(f'rf_finr32_{tag}_f', rseq)}\n{push_test}",
                 setup=size32, kind="Slab", note=f"recommended inline, {tag}, one size through the tables each call")


def mix_streams(k):
    """Pairs and batches of mixed classes: the class of each operation comes from one of the streams
    `tables::prepare_streams` writes, so how often an operation repeats the last one's word, and how
    predictably, is known. Base shape against the stale word."""
    streams = {0: "one class", 1: "cycle of 2", 2: "random of 2", 3: "random of 4", 4: "random of 8",
               5: "random of 16", 6: "same as last 7 times in 8", 7: "cycle of 8"}
    for st, what in streams.items():
        setup = HOIST_INPUTS + f"  mov x25, #({st + 1} << 18)\n  add x25, x22, x25\n  mov x24, #0\n"
        next_class = """  ldrh w5, [x25, x24, lsl #1]
  add x24, x24, #1
  and x24, x24, #0xffff
  add x3, x27, x5
"""
        for tag, mk_pop, mk_push in (("base", lambda p: pop(p), lambda p: push(p)),
                                     ("stale", lambda p: stale_pop(p, "x17"), lambda p: stale_push(p, "x17"))):
            k.kernel(f"m_{tag}_{st}", next_class + mk_pop(f"rf_m_{tag}{st}_a") + "\n" + mk_push(f"rf_m_{tag}{st}_f"),
                     setup=setup, kind="Slab", note=f"{tag}: pair of a class from: {what}")
            batch_body = f"""  mov x28, #0
  .balign 32
3:
{next_class}{mk_pop(f"rf_mb_{tag}{st}_a")}
  str x0, [x22, x28, lsl #3]
  add x28, x28, #1
  cmp x28, #32
  b.ne 3b
  sub x24, x24, #32
  and x24, x24, #0xffff
  mov x28, #0
  .balign 32
4:
{next_class}  ldr x0, [x22, x28, lsl #3]
{mk_push(f"rf_mb_{tag}{st}_f")}
  add x28, x28, #1
  cmp x28, #32
  b.ne 4b"""
            k.kernel(f"mb_{tag}_{st}", batch_body, setup=setup, kind="Slab", ops=32, note=f"{tag}: batch 32 of classes from: {what}")


def build(k):
    mix_streams(k)
    stale_base(k)
    placement(k)
    recommended(k)
    # ---- Which pair does the design's shape reach, one change at a time?
    base = {"rseq": "exe"}
    candidate(k, "base", "exe: cmp, [x, x], load first, mask register", base, base)
    candidate(k, "got", "got: __rseq_offset through the GOT", {"rseq": "got"}, {"rseq": "got"})
    candidate(k, "word", "word: __rseq_offset through a word of the routine's own", {"rseq": "word"}, {"rseq": "word"})
    candidate(k, "hoist", "rseq area's address in a register", {"rseq": "hoist"}, {"rseq": "hoist"})
    candidate(k, "tbnz", "cpu check: tbnz bit 31", {"cpu": "tbnz"}, {"cpu": "tbnz"})
    candidate(k, "ubfiz", "cpu check: ubfiz into a reserved top slab", {"cpu": "ubfiz"}, {"cpu": "ubfiz"})
    candidate(k, "nocheck", "no cpu check at all", {"cpu": "nocheck"}, {"cpu": "nocheck"})
    candidate(k, "add", "word's address by an add, then [x9]", {"addr": "add"}, {"addr": "add"})
    candidate(k, "addlsl", "word's address by add lsl, cmp keeps the raw number", {"addr": "addlsl", "cpu": "cmps"}, {"addr": "addlsl", "cpu": "cmps"})
    candidate(k, "ubfizadd", "ubfiz, then an add, then [x9]", {"addr": "add", "cpu": "ubfiz"}, {"addr": "add", "cpu": "ubfiz"})
    candidate(k, "subfirst", "pop: sub, then the slot's load", {"order": "sub_first"})
    candidate(k, "pre", "pop: pre-index load", {"order": "pre"})
    candidate(k, "post", "push: post-index store", None, {"order": "post"})
    candidate(k, "prepost", "pop pre-index, push post-index", {"order": "pre"}, {"order": "post"})
    candidate(k, "maskimm", "mask immediate", {"mask": "imm"}, {"mask": "imm"})
    candidate(k, "armgot", "arm through a GOT slot", {"arm": "got"}, {"arm": "got"})

    # ---- A word kept in a scratch register between calls.
    stale_setup = HOIST_INPUTS
    for reg, tag in (("x17", "x17"), ("x15", "x15")):
        pair(k, f"s_{tag}", stale_pop(f"rf_s_{tag}_a", reg), stale_push(f"rf_s_{tag}_f", reg), setup=stale_setup, note=f"word's address stale in {reg}")
        calls(k, f"s_{tag}", stale_pop(f"rf_sc_{tag}_a", reg), stale_push(f"rf_sc_{tag}_f", reg), setup=stale_setup, note=f"word's address stale in {reg}")
        batch(k, f"s_{tag}", stale_pop(f"rf_sb_{tag}_a", reg), stale_push(f"rf_sb_{tag}_f", reg), setup=stale_setup, note=f"word's address stale in {reg}")
    # A taken branch between: what separates the two ops in the rename groups.
    k.raw("""
.pushsection .data.rel.ro, "aw"
.balign 16
.globl rf_rseq_word
rf_rseq_word:
  .quad __rseq_offset
.popsection
""")
