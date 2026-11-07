"""The C entry shapes and the slow path's routines, as out-of-line functions called from a loop.

`malloc` takes a size in x0 and returns a pointer in x0, so a sequence must not write x0 before its
commit: an abort restarts from the arming store with the arguments as they were.

Options:
  rseq   word (a routine's own word holding __rseq_offset's address), got, exe
  cpu    cmp, ubfiz
  masks  table (adrp+add of the masks, then an indexed load), slab (8 bytes a position in slab 0's
         upper half, [word + 2048]), packed (the size table holds word offset and mask together)
  free   table (the region table byte, then the mask), packed

Tables are `tables::prepare_c`'s: every table is at a page offset of 2048 in rf_tables, offsets in
the constants below.
"""
from floors import batch, rseq_section
from routines import area, arming, lean_group, lean_prologue

SIZES = 2048
CALLOC = 8192 + 2048
MASKS = 16384 + 2048
PUBLISHED = 16384 + 1024
REGIONS = 16384 + 4096 + 2048
SIZED = 65536 + 2048
SIZES32 = 131072 + 2048
REGIONS32 = 163840 + 2048


def cpu_steps(cpu, slow):
    if cpu == "cmp":
        return f"""  ldr w8, [x10, #4]
  cmp w8, #2, lsl #12
  b.hs {slow}
  lsl x8, x8, #12"""
    if cpu == "ubfiz":
        return """  ldr w8, [x10, #4]
  ubfiz x8, x8, #12, #13"""
    raise ValueError(cpu)


def table(reg, off):
    return f"""  adrp {reg}, rf_tables + {off}
  add {reg}, {reg}, :lo12:rf_tables + {off}"""


def word_and_mask(masks, cls="x5"):
    """From the class's position in `cls` (a byte), the hot word in x1, its address operand, and
    the mask in w6. Returns text; the word's operand is [x3, x8]. `scaled` forms the CPU's slab's
    address first and indexes it by the position: it has no ALU op after the table's load."""
    if masks == "scaled":
        return f"""  adrp x13, rf_slabs
  add x3, x13, x8
  ldr x1, [x3, {cls}, lsl #3]
{table("x14", MASKS)}
  ldr x6, [x14, {cls}, lsl #3]"""
    if masks == "table":
        return f"""  adrp x13, rf_slabs
  add x3, x13, {cls}, lsl #3
  ldr x1, [x3, x8]
{table("x14", MASKS)}
  ldr x6, [x14, {cls}, lsl #3]"""
    if masks == "slab":
        return f"""  adrp x13, rf_slabs
  add x3, x13, {cls}, lsl #3
  ldr x1, [x3, x8]
  ldr x6, [x3, #2048]"""
    raise ValueError(masks)


def malloc_body(p, rseq="word", cpu="cmp", masks="table", arm="adrp"):
    if masks == "packed":
        lookup = f"""{table("x12", SIZES32)}
  ldr w5, [x12, x2, lsl #2]
  adrp x13, rf_slabs
  and x7, x5, #0xffff
  add x3, x13, x7
  ldr x1, [x3, x8]
  lsr w6, w5, #16"""
    else:
        lookup = f"""{table("x12", SIZES)}
  ldrb w5, [x12, x2]
{word_and_mask(masks)}"""
    return f"""{area(rseq)}
{p}_restart:
{arming(p, arm)}
{p}_start:
{cpu_steps(cpu, p + "_slow")}
  cmp x0, #8, lsl #12
  b.hi {p}_slow
  add x2, x0, #7
  lsr x2, x2, #3
{lookup}
  tst w1, w6
  b.eq {p}_slow
  ldr x4, [x1, #-8]
  sub x1, x1, #8
  str x1, {"[x3, x5, lsl #3]" if masks == "scaled" else "[x3, x8]"}
{p}_end:
{rseq_section(None, "pop", p)}
  mov x0, x4
  ret
  .subsection 1
{p}_slow:
  brk #2
  .subsection 0"""


def region_index(p, reg="x4"):
    """The region index of x0 in `reg`, and to the slow path if it is out of range."""
    return f"""  adrp x14, rf_tables + {PUBLISHED}
  ldr w6, [x14, :lo12:rf_tables + {PUBLISHED}]
  ldr x7, [x14, :lo12:rf_tables + {PUBLISHED + 8}]
  lsr {reg}, x0, x6
  sub {reg}, {reg}, x7
  cmp {reg}, #1, lsl #12
  b.hs {p}_slow"""


def free_body(p, rseq="word", cpu="cmp", masks="table", arm="adrp"):
    if masks == "packed":
        lookup = f"""{table("x12", REGIONS32)}
  ldr w5, [x12, x4, lsl #2]
  adrp x13, rf_slabs
  and x7, x5, #0xffff
  add x3, x13, x7
  ldr x1, [x3, x8]
  lsr w6, w5, #16"""
    else:
        lookup = f"""{table("x12", REGIONS)}
  ldrb w5, [x12, x4]
{word_and_mask(masks)}"""
    return f"""{area(rseq)}
{p}_restart:
{arming(p, arm)}
{p}_start:
{cpu_steps(cpu, p + "_slow")}
  cbz x0, {p}_end
{region_index(p)}
{lookup}
  add x2, x1, #8
  tst w2, w6
  b.eq {p}_slow
  str x0, [x1]
  str x2, {"[x3, x5, lsl #3]" if masks == "scaled" else "[x3, x8]"}
{p}_end:
{rseq_section(None, "push", p)}
  ret
  .subsection 1
{p}_slow:
  brk #2
  .subsection 0"""


def free_sized_body(p, rseq="word", cpu="cmp", masks="table", arm="adrp"):
    """Class from the size, the address checks it. x1 is the size on entry, so the word uses x15."""
    return f"""{area(rseq)}
{p}_restart:
{arming(p, arm)}
{p}_start:
{cpu_steps(cpu, p + "_slow")}
{table("x12", SIZED)}
  and x2, x1, #0xffff
  ldrb w5, [x12, x2]
{region_index(p)}
{table("x12", REGIONS)}
  ldrb w7, [x12, x4]
  cmp w5, w7
  b.ne {p}_slow
  adrp x13, rf_slabs
{"  add x3, x13, x8" if masks == "scaled" else "  add x3, x13, x5, lsl #3"}
  ldr x15, {"[x3, x5, lsl #3]" if masks == "scaled" else "[x3, x8]"}
{table("x14", MASKS) if masks != "slab" else ""}
  {"ldr x6, [x14, x5, lsl #3]" if masks != "slab" else "ldr x6, [x3, #2048]"}
  add x2, x15, #8
  tst w2, w6
  b.eq {p}_slow
  str x0, [x15]
  str x2, {"[x3, x5, lsl #3]" if masks == "scaled" else "[x3, x8]"}
{p}_end:
{rseq_section(None, "push", p)}
  ret
  .subsection 1
{p}_slow:
  brk #2
  .subsection 0"""


def calloc_body(p, rseq="word", cpu="cmp", masks="table", arm="adrp", tail="stores"):
    """n in x0, size in x1, copied to x7 since the hot word's load writes x1 and an abort restarts
    with registers as they stand. The product goes in x15."""
    return f"""{area(rseq)}
  mov x7, x1
{p}_restart:
{arming(p, arm)}
{p}_start:
{cpu_steps(cpu, p + "_slow")}
  orr x2, x0, x7
  cmp x2, #16, lsl #12
  b.hs {p}_slow
  mul x15, x0, x7
  cmp x15, #1024
  b.hi {p}_slow
{table("x12", CALLOC)}
  ldrb w5, [x12, x15]
{word_and_mask(masks)}
  tst w1, w6
  b.eq {p}_slow
  ldr x4, [x1, #-8]
  sub x1, x1, #8
  str x1, {"[x3, x5, lsl #3]" if masks == "scaled" else "[x3, x8]"}
{p}_end:
{rseq_section(None, "pop", p)}
  mov x0, x4
  cmp x15, #8
  b.hi {p}_wide
  str xzr, [x0]
  ret
{p}_wide:
  stp xzr, xzr, [x0]
  cmp x15, #16
  b.ls {p}_done
  stp xzr, xzr, [x0, #16]
  cmp x15, #32
  b.ls {p}_done
  brk #3
{p}_done:
  ret
  .subsection 1
{p}_slow:
  brk #2
  .subsection 0"""


def slab_index(p):
    """x3 starts as `NO_CPU`, the answer if the number has no slab; x9 and x8 end as the operand of
    the slab word at the offset in x0."""
    return f"""  mov x3, #2
  ldr w8, [x10, #4]
  cmp w8, #2, lsl #12
  b.hs {p}_end
  lsl x8, x8, #12
  and x9, x0, #0xff8
  add x8, x8, x9
  adrp x9, rf_slabs"""


def slab_cas(p, rseq="word"):
    return f"""{lean_prologue(p, rseq)}
{slab_index(p)}
  ldr x3, [x9, x8]
  cmp x3, x1
  b.ne {p}_end
  str x2, [x9, x8]
{p}_end:
{lean_group(p)}
  mov x0, x3
  ret"""


def stack_push(p, rseq="word"):
    return f"""{lean_prologue(p, rseq)}
{slab_index(p)}
  ldr x3, [x9, x8]
  tbnz w3, #0, {p}_end
  str x3, [x1]
  str x1, [x9, x8]
{p}_end:
{lean_group(p)}
  mov x0, x3
  ret"""


def stack_pop(p, rseq="word"):
    return f"""{lean_prologue(p, rseq)}
{slab_index(p)}
  ldr x3, [x9, x8]
  tbnz w3, #0, {p}_end
  cbz x3, {p}_end
  ldr x4, [x3]
  str x4, [x9, x8]
{p}_end:
{lean_group(p)}
  mov x0, x3
  ret"""


def current_cpu(rseq):
    return f"""{area(rseq)}
  ldr w0, [x10, #4]
  ret"""


def fn(k, name, body):
    k.raw(f"""
.text
.balign 64
.globl {name}
.type {name},%function
{name}:
{body}
.size {name}, .-{name}
""")


def plt(k, name, target):
    """A stub of the linker's shape, and the slot it reads."""
    k.raw(f"""
.text
.balign 16
{name}_plt:
  adrp x16, {name}_slot
  ldr x17, [x16, :lo12:{name}_slot]
  add x16, x16, :lo12:{name}_slot
  br x17
.pushsection .data.rel.ro, "aw"
.balign 8
{name}_slot:
  .quad {target}
.popsection
""")


def got_call(name):
    return f"""  adrp x24, {name}_slot
  ldr x24, [x24, :lo12:{name}_slot]
  blr x24"""


# Published words and masks share a page, and the region table is 4096 bytes past the masks, as in
# `front::Front`: one `adrp` serves all three.
LEAN_FRONT = f"""  adrp x14, rf_tables + {PUBLISHED}"""
LEAN_MASKS = f"""  add x15, x14, :lo12:rf_tables + {MASKS}"""


def lean_slab():
    return """  lsl x8, x8, #12
  adrp x13, rf_slabs
  add x3, x13, x8"""


def lean_malloc(p):
    return f"""{lean_prologue(p)}
  ldr w8, [x10, #4]
  cmp w8, #2, lsl #12
  b.hs {p}_slow
  cmp x0, #8, lsl #12
  b.hi {p}_slow
  add x2, x0, #7
  lsr x2, x2, #3
{table("x12", SIZES)}
  ldrb w5, [x12, x2]
{lean_slab()}
  ldr x1, [x3, x5, lsl #3]
{LEAN_FRONT}
{LEAN_MASKS}
  ldr x6, [x15, x5, lsl #3]
  tst w1, w6
  b.eq {p}_slow
  ldr x4, [x1, #-8]
  sub x1, x1, #8
  str x1, [x3, x5, lsl #3]
{p}_end:
{lean_group(p)}
  mov x0, x4
  ret
  .subsection 1
{p}_slow:
  brk #2
  .subsection 0"""


def lean_region(p):
    return f"""{LEAN_FRONT}
  ldr w6, [x14, :lo12:rf_tables + {PUBLISHED}]
  ldr x7, [x14, :lo12:rf_tables + {PUBLISHED + 8}]
  lsr x4, x0, x6
  sub x4, x4, x7
  cmp x4, #1, lsl #12
  b.hs {p}_slow
{LEAN_MASKS}
  add x12, x15, #{REGIONS - MASKS}"""


def lean_free(p):
    return f"""{lean_prologue(p)}
  ldr w8, [x10, #4]
  cmp w8, #2, lsl #12
  b.hs {p}_slow
  cbz x0, {p}_end
{lean_slab()}
{lean_region(p)}
  ldrb w5, [x12, x4]
  ldr x1, [x3, x5, lsl #3]
  ldr x6, [x15, x5, lsl #3]
  add x2, x1, #8
  tst w2, w6
  b.eq {p}_slow
  str x0, [x1]
  str x2, [x3, x5, lsl #3]
{p}_end:
{lean_group(p)}
  ret
  .subsection 1
{p}_slow:
  brk #2
  .subsection 0"""


def lean_free_swapped(p):
    """`lean_free` with the region index, which an ALU just made, as the table load's base: a base
    takes an ALU's result a cycle sooner than an index does."""
    return lean_free(p).replace("ldrb w5, [x12, x4]", "ldrb w5, [x4, x12]")


def lean_free_sized(p):
    return f"""{lean_prologue(p)}
  ldr w8, [x10, #4]
  cmp w8, #2, lsl #12
  b.hs {p}_slow
{lean_slab()}
{table("x2", SIZED)}
  and x9, x1, #0xffff
  ldrb w5, [x2, x9]
{lean_region(p)}
  ldrb w7, [x12, x4]
  cmp w5, w7
  b.ne {p}_slow
  ldr x9, [x3, x5, lsl #3]
  ldr x6, [x15, x5, lsl #3]
  add x2, x9, #8
  tst w2, w6
  b.eq {p}_slow
  str x0, [x9]
  str x2, [x3, x5, lsl #3]
{p}_end:
{lean_group(p)}
  ret
  .subsection 1
{p}_slow:
  brk #2
  .subsection 0"""


def lean_calloc(p, overflow="orr"):
    """n in x0, size in x1, both left alone for the slow path and the zeroing tail. `orr` turns away
    anything with a large argument, `umulh` exactly the products that overflow."""
    check = {
        "orr": f"  orr x2, x0, x1\n  cmp x2, #16, lsl #12\n  b.hs {p}_slow",
        "umulh": f"  umulh x2, x0, x1\n  cbnz x2, {p}_slow",
    }[overflow]
    return f"""{lean_prologue(p)}
  ldr w8, [x10, #4]
  cmp w8, #2, lsl #12
  b.hs {p}_slow
{check}
  mul x9, x0, x1
  cmp x9, #1024
  b.hi {p}_slow
{table("x12", CALLOC)}
  ldrb w5, [x12, x9]
{lean_slab()}
  ldr x2, [x3, x5, lsl #3]
{LEAN_FRONT}
{LEAN_MASKS}
  ldr x6, [x15, x5, lsl #3]
  tst w2, w6
  b.eq {p}_slow
  ldr x4, [x2, #-8]
  sub x2, x2, #8
  str x2, [x3, x5, lsl #3]
{p}_end:
{lean_group(p)}
  mov x0, x4
  cmp x9, #8
  b.hi {p}_wide
  str xzr, [x0]
  ret
{p}_wide:
  stp xzr, xzr, [x0]
  cmp x9, #16
  b.ls {p}_done
  stp xzr, xzr, [x0, #16]
  cmp x9, #32
  b.ls {p}_done
  brk #3
{p}_done:
  ret
  .subsection 1
{p}_slow:
  brk #2
  .subsection 0"""


def build_lean(k):
    malloc, free, sized, calloc = "rf_l_malloc", "rf_l_free", "rf_l_sized", "rf_l_calloc"
    fn(k, malloc, lean_malloc(malloc))
    fn(k, free, lean_free(free))
    fn(k, sized, lean_free_sized(sized))
    fn(k, calloc, lean_calloc(calloc))
    swapped = "rf_l_free_sw"
    fn(k, swapped, lean_free_swapped(swapped))
    k.kernel("cl_sw_d", f"  mov x0, #32\n  bl {malloc}\n  bl {swapped}", kind="Slab", note="lean malloc, free with the region index as base")
    k.kernel("cl_sw_mix", f"""  ldrh w26, [x25, x24, lsl #1]
  add x24, x24, #1
  and x24, x24, #63
  mov x0, x26
  bl {malloc}
  bl {swapped}""", setup="  mov x25, #(9 << 18)\n  add x25, x22, x25\n  mov x24, #0\n", kind="Slab", note="the same, 64 sizes")
    plt(k, malloc, malloc)
    plt(k, free, free)
    plt(k, sized, sized)
    size = "  mov x0, #32"
    mix_setup = "  mov x25, #(9 << 18)\n  add x25, x22, x25\n  mov x24, #0\n"
    next_size = """  ldrh w26, [x25, x24, lsl #1]
  add x24, x24, #1
  and x24, x24, #63
  mov x0, x26
"""
    # `mix` cycles through 64 sizes up to 512 bytes. A pair is pop then push of one object.
    forms = (("d", "", ""), ("p", "_plt", ""))
    for tag, suffix, _ in forms:
        k.kernel(f"cl_free_{tag}", f"{size}\n  bl {malloc}{suffix}\n  bl {free}{suffix}", kind="Slab", note=f"lean malloc, free, 32 bytes, {tag}")
        k.kernel(f"cl_sized_{tag}", f"{size}\n  bl {malloc}{suffix}\n  mov x1, #32\n  bl {sized}{suffix}", kind="Slab", note=f"lean malloc, sized free, 32 bytes, {tag}")
        k.kernel(f"cl_free_mix_{tag}", f"{next_size}  bl {malloc}{suffix}\n  bl {free}{suffix}", setup=mix_setup, kind="Slab", note=f"lean malloc, free, 64 sizes, {tag}")
        k.kernel(f"cl_sized_mix_{tag}", f"{next_size}  bl {malloc}{suffix}\n  mov x1, x26\n  bl {sized}{suffix}", setup=mix_setup, kind="Slab", note=f"lean malloc, sized free, 64 sizes, {tag}")
    k.kernel("cl_free_g", f"{size}\n{got_call(malloc)}\n{got_call(free)}", kind="Slab", note="lean malloc, free, 32 bytes, through the GOT")
    k.kernel("cl_sized_g", f"{size}\n{got_call(malloc)}\n  mov x1, #32\n{got_call(sized)}", kind="Slab", note="lean malloc, sized free, 32 bytes, through the GOT")
    k.kernel("cl_free_b", f"""  mov x24, #0
  .balign 32
3:
  mov x0, #32
  bl {malloc}
  str x0, [x22, x24, lsl #3]
  add x24, x24, #1
  cmp x24, #32
  b.ne 3b
  mov x24, #0
  .balign 32
4:
  ldr x0, [x22, x24, lsl #3]
  bl {free}
  add x24, x24, #1
  cmp x24, #32
  b.ne 4b""", ops=32, kind="Slab", note="lean malloc, free, batch of 32 of 32 bytes")
    exact = "rf_l_calloc_u"
    fn(k, exact, lean_calloc(exact, "umulh"))
    for n in (16, 32):
        k.kernel(f"cl_calloc_u_{n}", f"  mov x0, #1\n  mov x1, #{n}\n  bl {exact}\n  bl {free}", kind="Slab",
                 note=f"lean calloc(1, {n}), overflow by umulh, free")
    for n in (16, 32):
        k.kernel(f"cl_calloc_{n}", f"  mov x0, #1\n  mov x1, #{n}\n  bl {calloc}\n  bl {free}", kind="Slab", note=f"lean calloc(1, {n}) and free")
    # The sized comparison for the plainer routines, in the mixed loop.
    for tag in ("base", "scaled"):
        k.kernel(f"cs_{tag}_mix", f"{next_size}  bl rf_c_malloc_{tag}\n  mov x1, x26\n  bl rf_c_frees_{tag}", setup=mix_setup, kind="Slab", note=f"C pair with a sized free, 64 sizes: {tag}")


def build(k):
    variants = {
        "base": {},
        "exe": {"rseq": "exe"},
        "got": {"rseq": "got"},
        "ubfiz": {"cpu": "ubfiz"},
        "slab": {"masks": "slab"},
        "packed": {"masks": "packed"},
        "ubslab": {"cpu": "ubfiz", "masks": "slab"},
        "ubpacked": {"cpu": "ubfiz", "masks": "packed"},
        "scaled": {"masks": "scaled"},
        "ubscaled": {"cpu": "ubfiz", "masks": "scaled"},
    }
    for tag, v in variants.items():
        mname, fname = f"rf_c_malloc_{tag}", f"rf_c_free_{tag}"
        fn(k, mname, malloc_body(mname, **v))
        fn(k, fname, free_body(fname, **v))
        plt(k, mname, mname)
        plt(k, fname, fname)
        size = "  mov x0, #32"
        k.kernel(f"cm_{tag}_d", f"{size}\n  bl {mname}\n  bl {fname}", kind="Slab", note=f"C pair of 32 bytes, direct: {tag}")
        k.kernel(f"cm_{tag}_p", f"{size}\n  bl {mname}_plt\n  bl {fname}_plt", kind="Slab", note=f"C pair of 32 bytes, PLT: {tag}")
        k.kernel(f"cm_{tag}_g", f"{size}\n{got_call(mname)}\n{got_call(fname)}", kind="Slab", note=f"C pair of 32 bytes, through the GOT: {tag}")
        # Mixed sizes from a stream; x24's the index, x25 the sizes, as bytes.
        mix_setup = "  mov x25, #(9 << 18)\n  add x25, x22, x25\n  mov x24, #0\n"
        k.kernel(f"cm_{tag}_mix", f"""  ldrh w0, [x25, x24, lsl #1]
  add x24, x24, #1
  and x24, x24, #63
  bl {mname}
  bl {fname}""", setup=mix_setup, kind="Slab", note=f"C pair, 64 sizes in a cycle: {tag}")
        k.kernel(f"cm_{tag}_b", f"""  mov x24, #0
  .balign 32
3:
  mov x0, #32
  bl {mname}
  str x0, [x22, x24, lsl #3]
  add x24, x24, #1
  cmp x24, #32
  b.ne 3b
  mov x24, #0
  .balign 32
4:
  ldr x0, [x22, x24, lsl #3]
  bl {fname}
  add x24, x24, #1
  cmp x24, #32
  b.ne 4b""", ops=32, kind="Slab", note=f"C batch of 32 of 32 bytes, direct: {tag}")
        # What the caller's own work shows: a width-bound caller with N instructions per pair.
        for n in (0, 16, 32, 64):
            nops = "\n".join("  nop" for _ in range(n))
            k.kernel(f"cm_{tag}_w{n}", f"{size}\n  bl {mname}\n{nops}\n  bl {fname}\n{nops}", kind="Slab", note=f"C pair, {2 * n} nops between: {tag}")

    # Sized free.
    for tag, v in {"base": {}, "ubfiz": {"cpu": "ubfiz"}, "slab": {"masks": "slab"}, "scaled": {"masks": "scaled"}, "ubscaled": {"cpu": "ubfiz", "masks": "scaled"}}.items():
        mname, fname = f"rf_c_malloc_{'base' if tag == 'base' else tag}", f"rf_c_frees_{tag}"
        fn(k, fname, free_sized_body(fname, **v))
        k.kernel(f"cs_{tag}", f"  mov x0, #32\n  bl {mname}\n  mov x1, #32\n  bl {fname}", kind="Slab", note=f"C pair with a sized free: {tag}")
    # calloc, for 16 and 32 bytes: the tail zeroes.
    for tag, v in {"base": {}, "ubfiz": {"cpu": "ubfiz"}, "slab": {"masks": "slab"}, "scaled": {"masks": "scaled"}, "ubscaled": {"cpu": "ubfiz", "masks": "scaled"}}.items():
        cname = f"rf_c_calloc_{tag}"
        fn(k, cname, calloc_body(cname, **v))
        fname = f"rf_c_free_{'base' if tag == 'base' else tag}"
        for size in (16, 32):
            k.kernel(f"cc_{tag}_{size}", f"  mov x0, #1\n  mov x1, #{size}\n  bl {cname}\n  bl {fname}", kind="Slab", note=f"calloc(1, {size}) and free: {tag}")

    # The slow path's routines, alone and on the constant class's word.
    for tag, rseq in (("word", "word"), ("exe", "exe")):
        cas, sp, so, cc = f"rf_cas_{tag}", f"rf_spush_{tag}", f"rf_spop_{tag}", f"rf_ccpu_{tag}"
        fn(k, cas, slab_cas(cas, rseq=rseq))
        fn(k, sp, stack_push(sp, rseq=rseq))
        fn(k, so, stack_pop(so, rseq=rseq))
        fn(k, cc, current_cpu(rseq))
        k.kernel(f"sl_cas_ok_{tag}", f"""  mov x0, #152
  mov x1, x25
  mov x2, x25
  bl {cas}""", setup="  ldr x25, [x20, #152]", kind="Slab", note="slab_cas that succeeds, the same word")
        k.kernel(f"sl_cas_no_{tag}", f"""  mov x0, #152
  mov x1, #0
  mov x2, x25
  bl {cas}""", setup="  ldr x25, [x20, #152]", kind="Slab", note="slab_cas that fails")
        k.kernel(f"sl_stack_{tag}", f"""  mov x0, #96
  bl {so}
  cbz x0, 9f
  mov x1, x0
  mov x0, #96
  bl {sp}""", setup="""  add x25, x22, #2048
  str x25, [x20, #96]
  str xzr, [x25]""", kind="Slab", note="slab_stack_pop then slab_stack_push of one magazine")
        k.kernel(f"sl_cpu_{tag}", f"  bl {cc}", kind="Slab", note="current_cpu out of line")
    k.kernel("sl_cpu_inl", "  mrs x10, tpidr_el0\n  add x10, x10, x21\n  ldr w0, [x10, #4]", kind="Slab", note="current_cpu inline, the offset hoisted")
    build_lean(k)
