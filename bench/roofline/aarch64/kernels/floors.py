"""Floors: the cheapest code for the job, as a pair, through two calls, and in batches.

An allocation leaves the object in x0, and a free takes it there. Kinds: `Slab` has a magazine at
the constant class's word of the CPU's slab, x20 the slab and x23 the word's offset; `List` has a
list there. x24 onward are the kernel's own.
"""

MASK = "#0x1f0"
RSEQ_SIG = ".inst 0xd428bc00"


def pair(k, name, alloc, free, setup="", kind="Slab", note="", outside=""):
    k.kernel(f"p_{name}", f"{alloc}\n{free}", setup=setup, kind=kind, note="pair: " + note, outside=outside)


def calls(k, name, alloc, free, setup="", kind="Slab", note=""):
    """Through two calls, each out of line and aligned."""
    fa, ff = f"rf_fa_{name}", f"rf_ff_{name}"
    outside = f""".balign 64
{fa}:
{alloc}
  ret
9:
  brk #1
.balign 64
{ff}:
{free}
  ret
9:
  brk #1
"""
    k.kernel(f"c_{name}", f"  bl {fa}\n  bl {ff}", setup=setup, kind=kind, note="two calls: " + note, outside=outside, align=64)


def batch(k, name, alloc, free, setup="", kind="Slab", note="", n=32, outside=""):
    body = f"""  mov x24, #0
  .balign 32
3:
{alloc}
  str x0, [x22, x24, lsl #3]
  add x24, x24, #1
  cmp x24, #{n}
  b.ne 3b
  mov x24, #0
  .balign 32
4:
  ldr x0, [x22, x24, lsl #3]
{free}
  add x24, x24, #1
  cmp x24, #{n}
  b.ne 4b"""
    k.kernel(f"b_{name}", body, setup=setup, kind=kind, ops=n, note=f"batch {n}: " + note, outside=outside)


def shapes(k, name, alloc, free, setup="", kind="Slab", note="", which="pcb", outside=""):
    if "p" in which:
        pair(k, name, alloc, free, setup, kind, note, outside=outside)
    if "c" in which:
        calls(k, name, alloc, free, setup, kind, note)
    if "b" in which:
        batch(k, name, alloc, free, setup, kind, note, outside=outside if "p" not in which else "")


def rseq_section(k, stem, body_label_prefix):
    """The descriptor and abort stub of a sequence whose restart, start and end labels start with
    the prefix."""
    p = body_label_prefix
    return f"""  .pushsection __rseq_cs, "aw"
  .balign 32
{p}_cs:
  .long 0, 0
  .quad {p}_start, {p}_end - {p}_start, {p}_abort
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


def build(k):
    # ---- A magazine with everything held in registers: the word's address in x25, never written.
    hoist = "  add x25, x20, x23\n"
    bare_pop = f"""  ldr x1, [x25]
  tst w1, {MASK}
  b.eq 9f
  ldr x0, [x1, #-8]
  sub x1, x1, #8
  str x1, [x25]"""
    bare_push = f"""  ldr x1, [x25]
  add x2, x1, #8
  tst w2, {MASK}
  b.eq 9f
  str x0, [x1]
  str x2, [x25]"""
    shapes(k, "bare", bare_pop, bare_push, setup=hoist, note="bare magazine, the word's address hoisted")

    # ---- The word through a register offset, nothing else: no rseq, no CPU number.
    regoff_pop = f"""  ldr x1, [x20, x23]
  tst w1, {MASK}
  b.eq 9f
  ldr x0, [x1, #-8]
  sub x1, x1, #8
  str x1, [x20, x23]"""
    regoff_push = f"""  ldr x1, [x20, x23]
  add x2, x1, #8
  tst w2, {MASK}
  b.eq 9f
  str x0, [x1]
  str x2, [x20, x23]"""
    shapes(k, "bareidx", regoff_pop, regoff_push, note="bare magazine, word through [x, x]")

    # ---- The word's address recomputed every call, as a call must.
    addr_pop = f"""  add x9, x20, x23
  ldr x1, [x9]
  tst w1, {MASK}
  b.eq 9f
  ldr x0, [x1, #-8]
  sub x1, x1, #8
  str x1, [x9]"""
    addr_push = f"""  add x9, x20, x23
  ldr x1, [x9]
  add x2, x1, #8
  tst w2, {MASK}
  b.eq 9f
  str x0, [x1]
  str x2, [x9]"""
    shapes(k, "bareadd", addr_pop, addr_push, note="bare magazine, word's address by an add each call")

    # ---- A thread-local list and magazine: the base recomputed from the thread pointer.
    tls_setup = "  mrs x8, tpidr_el0\n  add x25, x20, x23\n  sub x25, x25, x8\n"
    tls_list_pop = """  mrs x8, tpidr_el0
  add x8, x8, x25
  ldr x0, [x8]
  cbz x0, 9f
  ldr x1, [x0]
  str x1, [x8]"""
    tls_list_push = """  mrs x8, tpidr_el0
  add x8, x8, x25
  ldr x1, [x8]
  str x1, [x0]
  str x0, [x8]"""
    shapes(k, "tlslist", tls_list_pop, tls_list_push, setup=tls_setup, kind="List", note="thread-local list through a base register")
    tls_mag_pop = f"""  mrs x8, tpidr_el0
  add x8, x8, x25
  ldr x1, [x8]
  tst w1, {MASK}
  b.eq 9f
  ldr x0, [x1, #-8]
  sub x1, x1, #8
  str x1, [x8]"""
    tls_mag_push = f"""  mrs x8, tpidr_el0
  add x8, x8, x25
  ldr x1, [x8]
  add x2, x1, #8
  tst w2, {MASK}
  b.eq 9f
  str x0, [x1]
  str x2, [x8]"""
    shapes(k, "tlsmag", tls_mag_pop, tls_mag_push, setup=tls_setup, note="thread-local magazine through a base register")

    # ---- A list with everything hoisted.
    shapes(k, "barelist", """  ldr x0, [x25]
  cbz x0, 9f
  ldr x1, [x0]
  str x1, [x25]""", """  ldr x1, [x25]
  str x1, [x0]
  str x0, [x25]""", setup=hoist, kind="List", note="bare list, the head's address hoisted")

    # ---- Per-CPU under rseq, in the plainest form: rseq area's address hoisted in x26, slabs in
    #      x27. A floor, so nothing here reads __rseq_offset.
    pcpu_setup = """  mrs x26, tpidr_el0
  add x26, x26, x21
  ldr w8, [x26, #4]
  sub x27, x20, x8, lsl #12
  add x28, x27, x23
"""

    def pcpu(name, pop_body, push_body, kind, note):
        def pop_text(p):
            return f"""{p}_restart:
  adrp x10, {p}_cs
  add x10, x10, :lo12:{p}_cs
  str x10, [x26, #8]
{p}_start:
  ldr w8, [x26, #4]
  lsl x8, x8, #12
{pop_body}
{p}_end:
{rseq_section(k, "pop", p)}"""

        def push_text(p):
            return f"""{p}_restart:
  adrp x10, {p}_cs
  add x10, x10, :lo12:{p}_cs
  str x10, [x26, #8]
{p}_start:
  ldr w8, [x26, #4]
  lsl x8, x8, #12
{push_body}
{p}_end:
{rseq_section(k, "push", p)}"""

        pair(k, name, pop_text(f"rf_p_{name}_a"), push_text(f"rf_p_{name}_f"), pcpu_setup, kind, note)
        calls(k, name, pop_text(f"rf_c_{name}_a"), push_text(f"rf_c_{name}_f"), pcpu_setup, kind, note)
        batch(k, name, pop_text(f"rf_b_{name}_a"), push_text(f"rf_b_{name}_f"), pcpu_setup, kind, note)

    pcpu("pcpulist", """  ldr x0, [x28, x8]
  cbz x0, 9f
  ldr x1, [x0]
  str x1, [x28, x8]""", """  ldr x1, [x28, x8]
  str x1, [x0]
  str x0, [x28, x8]""", "List", "per-CPU list under rseq, area and slabs hoisted")
    pcpu("pcpumag", f"""  ldr x1, [x28, x8]
  tst w1, {MASK}
  b.eq 9f
  ldr x0, [x1, #-8]
  sub x1, x1, #8
  str x1, [x28, x8]""", f"""  ldr x1, [x28, x8]
  add x2, x1, #8
  tst w2, {MASK}
  b.eq 9f
  str x0, [x1]
  str x2, [x28, x8]""", "Slab", "per-CPU magazine under rseq, area and slabs hoisted")

    # ---- tcmalloc's Pop and Push on aarch64, translated faithfully (percpu_tcmalloc.h): the slab
    #      pointer in a thread-local word with the top bit as "cached" flag (top byte ignored),
    #      32-bit header of 16-bit current and capacity, slots in units of 8 from the slab, and
    #      its prefetches after the commit. The address of the thread-local word and of `rseq_cs`
    #      are the compiler's, from `mrs`.
    tcm_setup = """  mrs x8, tpidr_el0
  sub x25, x20, x8
  add x25, x25, #4096
  orr x9, x20, #(1 << 63)
  str x9, [x20, #4096]
  mov x26, x21
  mov x27, #0xffff
"""

    def tcm_arm(p):
        return f"""{p}_restart:
  adrp x10, :got:{p}_cs
  ldr x10, [x10, :got_lo12:{p}_cs]
  mrs x11, tpidr_el0
  add x12, x11, x26
  str x10, [x12, #8]
{p}_start:
  ldr x13, [x11, x25]
  tbz x13, #63, 9f"""

    def tcm_pop(p, prefetch):
        tail = "  prfm pstl1keep, [x15, #-24]\n  prfm pldl1keep, [x9]" if prefetch else ""
        return f"""{tcm_arm(p)}
  ldr w14, [x13, x23, lsl #2]
  and w15, w14, #0xffff
  add w14, w14, w27
  add x15, x13, x15, lsl #3
  ldur x0, [x15, #-8]
  tbnz x0, #0, 9f
  ldur x9, [x15, #-16]
  str w14, [x13, x23, lsl #2]
{p}_end:
{rseq_section(k, "pop", p)}
{tail}"""

    def tcm_push(p, prefetch):
        tail = "  add x15, x15, #1\n  prfm pstl1keep, [x13, x15, lsl #3]" if prefetch else ""
        return f"""{tcm_arm(p)}
  ldr w14, [x13, x23, lsl #2]
  and w15, w14, #0xffff
  subs w14, w14, w27
  b.cc 9f
  str x0, [x13, x15, lsl #3]
  str w14, [x13, x23, lsl #2]
{p}_end:
{rseq_section(k, "push", p)}
{tail}"""

    for pf, tag, what in ((True, "tcm", "tcmalloc Pop and Push"), (False, "tcmnp", "tcmalloc Pop and Push, no prefetch")):
        pair(k, tag, tcm_pop(f"rf_p_{tag}_a", pf), tcm_push(f"rf_p_{tag}_f", pf), tcm_setup, "Tcm", what)
        calls(k, tag, tcm_pop(f"rf_c_{tag}_a", pf), tcm_push(f"rf_c_{tag}_f", pf), tcm_setup, "Tcm", what)
        batch(k, tag, tcm_pop(f"rf_b_{tag}_a", pf), tcm_push(f"rf_b_{tag}_f", pf), tcm_setup, "Tcm", what)
