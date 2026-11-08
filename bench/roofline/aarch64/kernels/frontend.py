"""The front end: loop size, position, taken and not-taken branches, calls."""


def adds(n):
    regs = ["x5", "x6", "x7", "x8", "x9", "x10"]
    return "\n".join(f"  add {regs[i % len(regs)]}, {regs[i % len(regs)]}, #1" for i in range(n))


def nops(n):
    return "\n".join("  nop" for _ in range(n))


def build(k):
    # Cycles against loop length in instructions, in nops with a fused loop branch: a pure front
    # end, loop start at a 64-byte boundary.
    for n in list(range(0, 33)) + [40, 48, 56, 62, 64, 72, 80, 96, 112, 126, 128, 160, 192, 256, 384, 512]:
        k.kernel(f"fe_len{n}", nops(n), align=256, note=f"{n} nops + subs/b.ne")

    # The same at each position in a 64-byte window and a 16 KiB page, for lengths around what a
    # routine would be: is there any cost in where the loop sits?
    for n in (2, 6, 14, 22, 30, 38):
        for pad in range(0, 32):
            k.kernel(f"fe_pos{n}_{pad * 4}", nops(n), align=128, pad=pad, note=f"{n} nops + subs/b.ne at byte {pad * 4} of 128")

    # Taken branches: k of them in a loop, each to the next instruction.
    for n in (1, 2, 3, 4, 6, 8, 12, 16, 24, 32, 48, 64, 96, 128, 192, 256, 512, 1024, 2048, 4096):
        body = "\n".join("  b 3f\n3:" for _ in range(n))
        k.kernel(f"fe_taken{n}", body, ops=n + 1, align=64, note=f"{n} taken branches + the loop's, per branch")
    # Spaced: a taken branch every 16, 32, 64, 128 or 256 bytes.
    for stride in (16, 32, 64, 128, 256):
        for n in (2, 4, 8, 16, 64, 128, 256):
            fill = nops(stride // 4 - 1)
            body = "\n".join(f"  b 3f\n{fill}\n3:" for _ in range(n))
            k.kernel(f"fe_gap{stride}_{n}", body, ops=n + 1, note=f"{n} taken branches {stride} bytes apart: the nops are skipped, per branch")
    # Where taken branches stop being free: by the bytes the loop spans, not the branches.
    for stride, counts in ((64, (56, 64, 72, 80, 88, 96, 112, 128)), (32, (112, 128, 160, 192, 224, 256)), (128, (28, 32, 36, 40, 48, 56, 64))):
        for n in counts:
            fill = nops(stride // 4 - 1)
            body = "\n".join(f"  b 3f\n{fill}\n3:" for _ in range(n))
            k.kernel(f"fe_span{stride}_{n}", body, ops=n + 1, note=f"{n} taken branches {stride} bytes apart ({n * stride} bytes), per branch")
    # Calls from many sites to one leaf, 64 bytes a site, against the same code with no call: what
    # a call costs once the sites span more than the loop above can hold. A far leaf is 2 MiB away.
    k.raw(""".section .text.rf_far, "ax"
.balign 64
rf_far_pad:
  .skip 2 * 1024 * 1024
.balign 64
rf_far_leaf:
  ret
.text
.balign 64
rf_near_leaf:
  ret
""")
    for n in (8, 32, 64, 128, 256, 1024, 4096):
        for tag, target in (("near", "rf_near_leaf"), ("far", "rf_far_leaf")):
            body = "\n".join(f"  bl {target}\n{nops(15)}" for _ in range(n))
            k.kernel(f"fe_sites_{tag}{n}", body, ops=n, align=64, note=f"{n} sites a pass, bl to a {tag} leaf and 15 nops, per site")
        body = "\n".join(f"  nop\n{nops(15)}" for _ in range(n))
        k.kernel(f"fe_sites_none{n}", body, ops=n, align=64, note=f"{n} sites a pass, 16 nops, per site")

    # A taken branch every N straight instructions: what a run of N costs.
    for n in (1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 20, 24, 32, 48, 64):
        body = "\n".join([f"{nops(n)}\n  b 3f\n3:" for _ in range(8)])
        k.kernel(f"fe_run{n}", body, ops=8, note=f"8 runs a pass of {n} nops and a taken branch, per run")

    # Not-taken branches.
    never = {
        "cbnz": "  cbnz xzr, 9f",
        "tbnz": "  tbnz xzr, #1, 9f",
        "bcc": "  b.vs 9f",
    }
    for name, br in never.items():
        for n in (1, 2, 3, 4, 6, 8, 12, 16):
            k.kernel(f"fe_nt_{name}{n}", "\n".join([br] * n), setup="  cmp x20, x20", ops=1, note=f"{n} never-taken {name} a pass")
    k.kernel("fe_nt_mix", "  cbnz xzr, 9f\n  nop\n  nop\n  nop", setup="  cmp x20, x20", note="a never-taken branch then 3 nops")

    # Calls.
    leaf = """.balign 64
rf_leaf:
  ret
.balign 64
rf_leaf_b:
  ret
.balign 64
rf_leaf_nop4:
  nop
  nop
  nop
  ret
"""
    k.kernel("fe_call1", "  bl rf_leaf", note="bl + ret", outside=leaf)
    k.kernel("fe_call2", "  bl rf_leaf\n  bl rf_leaf", note="2 x (bl + ret)")
    k.kernel("fe_call2b", "  bl rf_leaf\n  bl rf_leaf_b", note="2 x (bl + ret), different leaves")
    k.kernel("fe_call4", "  bl rf_leaf\n  bl rf_leaf\n  bl rf_leaf\n  bl rf_leaf", note="4 x (bl + ret)")
    k.kernel("fe_call_work", "  bl rf_leaf_nop4", note="bl + 3 nops + ret")
    work8 = "\n".join("  add x5, x5, #1" for _ in range(8))
    k.kernel("fe_call_chain8", f"  bl rf_leaf\n{work8}", note="bl + ret beside 8 dependent adds")
    k.kernel("fe_chain8", work8, note="8 dependent adds")
    k.kernel("fe_icall1", "  blr x24", setup="  adrp x24, rf_leaf\n  add x24, x24, :lo12:rf_leaf", note="blr + ret")
    k.kernel("fe_icall2", "  blr x24\n  blr x24", setup="  adrp x24, rf_leaf\n  add x24, x24, :lo12:rf_leaf", note="2 x (blr + ret)")
    k.kernel("fe_iret2", "  blr x24\n  blr x25", setup="  adrp x24, rf_leaf\n  add x24, x24, :lo12:rf_leaf\n  adrp x25, rf_leaf_b\n  add x25, x25, :lo12:rf_leaf_b", note="2 blr, different targets")
    k.kernel("fe_br_tail", "  bl rf_tail", note="bl to a stub that tail-branches to the leaf", outside=""".balign 64
rf_tail:
  b rf_leaf
""")
    # The two ways a dynamic call goes: the PLT stub of the linker, and blr through the GOT.
    k.kernel("fe_plt1", "  bl rf_plt", note="bl to a PLT-shaped stub, then ret", outside=""".balign 16
rf_plt:
  adrp x16, rf_plt_slot
  ldr x17, [x16, :lo12:rf_plt_slot]
  add x16, x16, :lo12:rf_plt_slot
  br x17
.pushsection .data.rel.ro, "aw"
.balign 8
rf_plt_slot:
  .quad rf_leaf
.popsection
""")
    k.kernel("fe_plt2", "  bl rf_plt\n  bl rf_plt", note="2 calls through the PLT-shaped stub")
    k.kernel("fe_got1", "  adrp x24, rf_plt_slot\n  ldr x24, [x24, :lo12:rf_plt_slot]\n  blr x24", note="blr through the GOT-shaped slot")
    k.kernel("fe_got2", "  adrp x24, rf_plt_slot\n  ldr x24, [x24, :lo12:rf_plt_slot]\n  blr x24\n  adrp x24, rf_plt_slot\n  ldr x24, [x24, :lo12:rf_plt_slot]\n  blr x24", note="2 blr through the GOT-shaped slot")
    # A function of N independent adds, called: does a call hide work, and is there a cliff?
    fns = []
    for n in (0, 1, 2, 3, 4, 6, 8, 10, 12, 14, 16, 20, 24, 28, 32, 40, 48, 64):
        fns.append(f""".balign 64
rf_fn{n}:
{adds(n)}
  ret
""")
        k.kernel(f"fe_fn{n}", f"  bl rf_fn{n}\n  bl rf_fn{n}", ops=2, note=f"2 calls of a function of {n} adds, per call", outside=fns[-1])
    # The same at each offset in a line: a routine's own alignment.
    for n in (12, 24, 36):
        for off in (0, 4, 8, 16, 24, 32, 40, 48, 56, 60, 64, 96, 124):
            name = f"rf_fnp{n}_{off}"
            k.kernel(f"fe_fnp{n}_{off}", f"  bl {name}\n  bl {name}", ops=2, note=f"2 calls of {n} adds at byte {off} of 128", outside=f""".balign 128
  .skip {off}
{name}:
{adds(n)}
  ret
""")
    # Mispredicted branches: an always-different direction is too easy; use a long pseudo-random
    # pattern from a linear congruence, per mispredicted branch against the same code never taken.
    k.kernel("fe_mis_rand", """  add x5, x5, x6
  ror x5, x5, #17
  tbz x5, #7, 3f
  nop
3:""", setup="  movz x5, #0x1234\n  movz x6, #0x9e37, lsl #16\n  movk x6, #0x79b9", note="a branch on a pseudo-random bit: half mispredicted")
    k.kernel("fe_mis_none", """  add x5, x5, x6
  ror x5, x5, #17
  tbz xzr, #7, 3f
  nop
3:""", setup="  movz x5, #0x1234\n  movz x6, #0x9e37, lsl #16\n  movk x6, #0x79b9", note="same work, the branch always taken")
    k.kernel("fe_csel", """  add x5, x5, x6
  ror x5, x5, #17
  tst x5, #0x80
  csel x7, x5, x6, eq""", setup="  movz x5, #0x1234\n  movz x6, #0x9e37, lsl #16\n  movk x6, #0x79b9", note="same work with a csel")
