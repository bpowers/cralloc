"""Widths, latencies, eliminated moves and which pairs fuse."""

REGS = ["x5", "x6", "x7", "x8", "x9", "x10", "x11", "x12", "x13", "x14", "x15", "x16", "x17",
        "x24", "x25", "x26"]


def widths(k):
    for n in range(1, 13):
        body = "\n".join(f"  add {REGS[i]}, {REGS[i]}, #1" for i in range(n))
        k.kernel(f"w_add{n}", body, note=f"{n} independent adds + subs/b.ne")
    for n in (2, 4, 6, 8, 10, 12, 14, 16):
        body = "\n".join("  nop" for _ in range(n))
        k.kernel(f"w_nop{n}", body, note=f"{n} nops + subs/b.ne")
    for n in (2, 4, 6, 8, 10, 12):
        body = "\n".join(f"  mov {REGS[i]}, {REGS[i + 1]}" for i in range(n))
        k.kernel(f"w_mov{n}", body, note=f"{n} movs between registers")
    for n in (2, 4, 6, 8, 10, 12):
        body = "\n".join(f"  mov {REGS[i]}, #{i + 1}" for i in range(n))
        k.kernel(f"w_movz{n}", body, note=f"{n} small immediate moves")
    for n in (2, 4, 6, 8, 10):
        body = "\n".join(f"  mov {REGS[i]}, #0" for i in range(n))
        k.kernel(f"w_mov0_{n}", body, note=f"{n} moves of zero")
    for n in (2, 4, 6, 8):
        body = "\n".join(f"  mov {REGS[i]}, xzr" for i in range(n))
        k.kernel(f"w_movxzr{n}", body, note=f"{n} moves from the zero register")
    for n in (2, 4, 6, 8):
        body = "\n".join(f"  movz {REGS[i]}, #0x1234, lsl #16\n  movk {REGS[i]}, #0x5678" for i in range(n // 2))
        k.kernel(f"w_movzk{n}", body, note=f"{n // 2} movz+movk pairs")
    for n in (2, 4, 6, 8):
        body = "\n".join(f"  adrp {REGS[i]}, rf_data" for i in range(n))
        k.kernel(f"w_adrp{n}", body, note=f"{n} adrp")
    for n in (2, 4, 6, 8):
        body = "\n".join(f"  adr {REGS[i]}, 2b" for i in range(n))
        k.kernel(f"w_adr{n}", body, note=f"{n} adr")
    for n in (1, 2, 3, 4, 6, 8):
        body = "\n".join(f"  mrs {REGS[i]}, tpidr_el0" for i in range(n))
        k.kernel(f"w_mrs{n}", body, note=f"{n} mrs tpidr_el0")
    for n in (1, 2, 3, 4, 6, 8):
        body = "\n".join(f"  add {REGS[i]}, {REGS[i]}, {REGS[i + 1]}, lsl #3" for i in range(n))
        k.kernel(f"w_addsh{n}", body, note=f"{n} add with shifted register")
    for n in (2, 4, 6, 8):
        body = "\n".join(f"  ubfiz {REGS[i]}, {REGS[i + 1]}, #12, #13" for i in range(n))
        k.kernel(f"w_ubfiz{n}", body, note=f"{n} ubfiz")
    for n in (1, 2, 3, 4, 5, 6, 7, 8):
        body = "\n".join(f"  ldr {REGS[i]}, [x20, #{8 * i}]" for i in range(n))
        k.kernel(f"w_ld{n}", body, note=f"{n} loads")
    for n in (1, 2, 3, 4, 5, 6):
        body = "\n".join(f"  str x5, [x20, #{8 * i + 256}]" for i in range(n))
        k.kernel(f"w_st{n}", body, note=f"{n} stores")
    for n in (1, 2, 3, 4):
        body = "\n".join(f"  ldp {REGS[2 * i]}, {REGS[2 * i + 1]}, [x20, #{16 * i}]" for i in range(n))
        k.kernel(f"w_ldp{n}", body, note=f"{n} ldp")
        body = "\n".join(f"  stp x5, x6, [x20, #{16 * i + 256}]" for i in range(n))
        k.kernel(f"w_stp{n}", body, note=f"{n} stp")
    for ld, st in ((1, 1), (2, 1), (3, 1), (1, 2), (2, 2), (3, 2), (4, 2), (2, 3), (4, 1)):
        body = "\n".join(f"  ldr {REGS[i]}, [x20, #{8 * i}]" for i in range(ld))
        body += "\n" + "\n".join(f"  str x17, [x20, #{8 * i + 256}]" for i in range(st))
        k.kernel(f"w_mix_l{ld}s{st}", body, note=f"{ld} loads and {st} stores, unrelated")


def latencies(k):
    k.kernel("l_add", "  add x5, x5, #1", note="add chain")
    k.kernel("l_add2", "  add x5, x5, #1\n  add x5, x5, #1", ops=2, note="add chain, 2 a pass, per add")
    k.kernel("l_addsh", "  add x5, x5, x6, lsl #3", note="add chain, shifted register")
    k.kernel("l_addsh2", "  add x5, x6, x5, lsl #3", note="add chain through the shifted operand")
    k.kernel("l_ubfiz", "  ubfiz x5, x5, #12, #13", note="ubfiz chain")
    k.kernel("l_ubfx", "  ubfx x5, x5, #0, #13", note="ubfx chain")
    k.kernel("l_lsl", "  lsl x5, x5, #1", note="lsl chain")
    k.kernel("l_and", "  and x5, x5, #0x1f0", note="and immediate chain")
    k.kernel("l_andr", "  and x5, x5, x6", note="and register chain")
    k.kernel("l_ands", "  ands x5, x5, x6", note="ands chain")
    k.kernel("l_adds", "  adds x5, x5, #1", note="adds chain")
    k.kernel("l_orr", "  orr x5, x5, x6", note="orr chain")
    k.kernel("l_mov", "  mov x6, x5\n  add x5, x6, #1", note="mov then add: eliminated moves cost 1")
    k.kernel("l_mov3", "  mov x6, x5\n  mov x7, x6\n  mov x8, x7\n  add x5, x8, #1", note="3 movs then add")
    k.kernel("l_madd", "  mul x5, x5, x6", note="mul chain")

    # Pointers to themselves: x5 stays the state block's address whichever word it reads.
    self_words = "\n".join(f"  str x20, [x20, #{off}]" for off in (0, 8, 16, 24, 32, 64))
    base = "  mov x5, x20\n"
    k.kernel("l_ld_base", "  ldr x5, [x5]", setup=self_words + "\n" + base, note="load chain [x]")
    k.kernel("l_ld_imm", "  ldr x5, [x5, #16]", setup=self_words + "\n" + base, note="load chain [x, #16]")
    k.kernel("l_ld_reg", "  ldr x5, [x5, x6]", setup=self_words + "\n" + base + "  mov x6, #16", note="load chain [x, x]")
    k.kernel("l_ld_regidx", "  ldr x5, [x6, x5]", setup=self_words + "\n" + base + "  mov x6, #0", note="load chain through the index of [x, x]")
    k.kernel("l_ld_regsh", "  ldr x5, [x5, x6, lsl #3]", setup=self_words + "\n" + base + "  mov x6, #2", note="load chain [x, x, lsl #3]")
    k.kernel("l_ld_uxtw", "  ldr x5, [x5, w6, uxtw]", setup=self_words + "\n" + base + "  mov x6, #16", note="load chain [x, w, uxtw]")
    k.kernel("l_ld_uxtw3", "  ldr x5, [x5, w6, uxtw #3]", setup=self_words + "\n" + base + "  mov x6, #2", note="load chain [x, w, uxtw #3]")
    k.kernel("l_ld_pre_data", "  ldr x6, [x5, #16]!\n  mov x5, x6", setup=self_words + "\n" + base, note="pre-index, data chain, then mov")
    k.kernel("l_ld_post_data", "  ldr x6, [x5], #16\n  mov x5, x6", setup=self_words + "\n" + base, note="post-index, data chain, then mov")
    k.kernel("l_ld_plain_data", "  ldr x6, [x5, #16]\n  mov x5, x6", setup=self_words + "\n" + base, note="plain, data chain, then mov")
    k.kernel("l_ld_pre_wb", "  ldr x6, [x5, #8]!\n  ldr x6, [x5, #-8]!", ops=2, setup=self_words + "\n" + base, note="pre-index, writeback chain, per load")
    k.kernel("l_ld_post_wb", "  ldr x6, [x5], #8\n  ldr x6, [x5], #-8", ops=2, setup=self_words + "\n" + base, note="post-index, writeback chain, per load")
    k.kernel("l_st_pre_wb", "  str x6, [x5, #8]!\n  str x6, [x5, #-8]!", ops=2, setup="  add x5, x20, #256", note="pre-index store, writeback chain, per store")
    k.kernel("l_st_post_wb", "  str x6, [x5], #8\n  str x6, [x5], #-8", ops=2, setup="  add x5, x20, #256", note="post-index store, writeback chain, per store")
    k.kernel("l_ld_ldp1", "  ldp x5, x6, [x5]", setup=self_words + "\n" + base, note="ldp, first register's chain")
    k.kernel("l_ld_ldp2", "  ldp x6, x5, [x5]", setup=self_words + "\n" + base, note="ldp, second register's chain")
    # Narrow loads read zero, so the add restores the address: load + 1.
    k.kernel("l_ld_x0", "  ldr x6, [x5, #256]\n  add x5, x20, x6", setup=base, note="ldr x then add: lat + 1")
    k.kernel("l_ld_w", "  ldr w6, [x5, #256]\n  add x5, x20, x6", setup=base, note="ldr w then add: lat + 1")
    k.kernel("l_ld_h", "  ldrh w6, [x5, #256]\n  add x5, x20, x6", setup=base, note="ldrh then add: lat + 1")
    k.kernel("l_ld_b", "  ldrb w6, [x5, #256]\n  add x5, x20, x6", setup=base, note="ldrb then add: lat + 1")
    k.kernel("l_ld_sw", "  ldrsw x6, [x5, #256]\n  add x5, x20, x6", setup=base, note="ldrsw then add: lat + 1")
    k.kernel("l_ld_b_reg", "  ldrb w6, [x5, x7]\n  add x5, x20, x6", setup=base + "  mov x7, #256", note="ldrb [x, x] then add: lat + 1")
    k.kernel("l_ld_w_sh", "  ldr w6, [x5, x7, lsl #2]\n  add x5, x20, x6", setup=base + "  mov x7, #64", note="ldr w [x, x, lsl #2] then add: lat + 1")
    k.kernel("l_chain_cpu", "  ldr w6, [x5, #256]\n  lsl x7, x6, #12\n  ldr x8, [x20, x7]\n  add x5, x20, x8", setup=base, note="cpu load, shift, word load, add: the CPU chain")
    k.kernel("l_chain_cpu2", "  ldr w6, [x5, #256]\n  ubfiz x7, x6, #12, #13\n  ldr x8, [x20, x7]\n  add x5, x20, x8", setup=base, note="the same through ubfiz")
    k.kernel("l_chain_cpu3", "  ldr w6, [x5, #256]\n  add x7, x20, x6, lsl #12\n  ldr x8, [x7]\n  add x5, x20, x8", setup=base, note="the same through add lsl")

    # A result with no inputs shows its latency only after the pipeline drains: isb, the result,
    # a chain of adds from it, isb. The add-only and load forms frame it.
    adds = "\n".join("  add x5, x5, #1" for _ in range(8))
    for name, first, what in (("nop", "  nop\n  mov x5, x6", "mov"),
                              ("mrs", "  mrs x5, tpidr_el0", "mrs tpidr_el0"),
                              ("cntvct", "  mrs x5, cntvct_el0", "mrs cntvct_el0"),
                              ("ldr", "  ldr x5, [x20]", "ldr"),
                              ("adrp", "  adrp x5, rf_data", "adrp"),
                              ("movz", "  mov x5, #5", "mov immediate")):
        k.kernel(f"l_isb_{name}", f"  isb\n{first}\n{adds}\n  isb", note=f"isb; {what}; 8 adds; isb")
    k.kernel("w_mrs_cntvct", "  mrs x5, cntvct_el0", note="mrs cntvct_el0 alone")
    k.kernel("w_mrs_cntvct_add8", "  mrs x5, cntvct_el0\n" + adds, note="mrs cntvct_el0 and an unrelated chain")
    k.kernel("w_mrs_tp_ld3", "  mrs x5, tpidr_el0\n  ldr x6, [x20]\n  ldr x7, [x20, #8]\n  ldr x8, [x20, #16]", note="mrs tpidr_el0 beside 3 loads")
    k.kernel("w_mrs_tp_chain", "  mrs x5, tpidr_el0\n  ldr x6, [x5, x21]\n  add x6, x6, #1\n  ldr x7, [x5, #-8]", note="mrs, then loads through it")


def fusion(k):
    """Each cell repeats a pair; a fused pair shows as one micro-op retired, and a body of many
    pairs shows how many go a cycle. The branch is never taken."""
    setups = "  mov x5, #1\n  mov x6, #1\n  mov x7, #2\n  mov x8, #0xff\n  adds xzr, x5, x7\n"
    pairs = {
        "cmp_r": ("  cmp x5, x7\n  b.eq 9f", "cmp reg + b.eq"),
        "cmp_i": ("  cmp x5, #7\n  b.eq 9f", "cmp imm + b.eq"),
        "cmp_hs": ("  cmp w5, #7\n  b.hs 9f", "cmp w imm + b.hs"),
        "tst_i": ("  tst x8, #0x100\n  b.ne 9f", "tst imm + b.ne"),
        "tst_r": ("  tst w8, w6\n  b.eq 9f", "tst reg + b.eq"),
        "ands": ("  ands x9, x8, x6\n  b.eq 9f", "ands + b.eq"),
        "subs": ("  subs x9, x5, #7\n  b.eq 9f", "subs + b.eq"),
        "adds": ("  adds x9, x5, #7\n  b.eq 9f", "adds + b.eq"),
        "ccmp": ("  ccmp x5, x7, #0, ne\n  b.eq 9f", "ccmp + b.eq"),
        "cmp_sep": ("  cmp x5, x7\n  nop\n  b.eq 9f", "cmp, nop, b.eq: not adjacent"),
        "tst_sep": ("  tst w8, w6\n  nop\n  b.eq 9f", "tst, nop, b.eq"),
        "cbz": ("  cbz x5, 9f", "cbz alone"),
        "tbnz": ("  tbnz x5, #8, 9f", "tbnz alone"),
        "adrp_add": ("  adrp x9, rf_data\n  add x9, x9, :lo12:rf_data", "adrp + add"),
        "adrp_ldr": ("  adrp x9, rf_data\n  ldr x9, [x9, :lo12:rf_data]", "adrp + ldr"),
        "movzk": ("  movz x9, #0x1234, lsl #16\n  movk x9, #0x5678", "movz + movk"),
        "add_ldr": ("  add x9, x20, #16\n  ldr x10, [x9]", "add + ldr through it"),
        "lsl_add": ("  lsl x9, x5, #12\n  add x9, x9, x6", "lsl + add"),
        "ldr_add": ("  ldr x9, [x20]\n  add x9, x9, #1", "ldr + add of its result"),
        "mrs_add": ("  mrs x9, tpidr_el0\n  add x9, x9, x6", "mrs + add"),
    }
    for name, (body, note) in pairs.items():
        for n in (1, 4):
            k.kernel(f"z_{name}_{n}", "\n".join([body] * n), setup=setups, ops=n, note=note + f", {n} a pass, per pair")


def ports(k):
    """Which instructions compete: each cell pairs two kinds, a pass at 1.00 means they fit."""
    br = "  cbnz xzr, 9f"
    mrs = "  mrs x5, tpidr_el0"
    adrp = "  adrp x6, rf_data"
    add = "  add x7, x7, #1"
    cases = {
        "br1": [br],
        "adrp1": [adrp],
        "mrs_br": [mrs, br],
        "mrs_adrp": [mrs, adrp],
        "br_adrp": [br, adrp],
        "mrs_br_adrp": [mrs, br, adrp],
        "mrs2": [mrs, mrs],
        "br2": [br, br],
        "adrp2": [adrp, adrp],
    }
    for name, parts in cases.items():
        k.kernel(f"w_port_{name}", "\n".join(parts), note="competing with a fused loop branch: " + " + ".join(p.strip() for p in parts))
    for n in (1, 2, 3, 4, 5):
        body = "\n".join([f"  add {REGS[i]}, {REGS[i]}, x20, lsl #3" for i in range(n)])
        k.kernel(f"w_addsh_ind{n}", body, note=f"{n} add lsl, each its own chain")
        body = "\n".join([f"  add x{5 + i}, x20, x21, lsl #3" for i in range(n)])
        k.kernel(f"w_addsh_free{n}", body, note=f"{n} add lsl of constants")
    # Load to ALU, load to store data, ALU to load, by operand: which forward path costs a cycle.
    cases = {
        "ld_add": "  ldr x6, [x5, #256]\n  add x5, x20, x6",
        "ld_sub": "  ldr x6, [x5, #256]\n  sub x5, x20, x6",
        "ld_orr": "  ldr x6, [x5, #256]\n  orr x5, x20, x6",
        "ld_eor": "  ldr x6, [x5, #256]\n  eor x5, x20, x6",
        "ld_and": "  ldr x6, [x5, #256]\n  and x7, x20, x6\n  orr x5, x20, x7",
        "ld_lsl": "  ldr x6, [x5, #256]\n  lsl x7, x6, #12\n  add x5, x20, x7",
        "ld_ubfiz": "  ldr x6, [x5, #256]\n  ubfiz x7, x6, #12, #13\n  add x5, x20, x7",
        "ld_addlsl": "  ldr x6, [x5, #256]\n  add x5, x20, x6, lsl #12",
        "ld_madd": "  ldr x6, [x5, #256]\n  madd x5, x6, x6, x20",
        "ld_cmp_csel": "  ldr x6, [x5, #256]\n  cmp x6, #0\n  csel x5, x20, x6, eq",
        "ld_idx": "  ldr x6, [x5, #256]\n  ldr x5, [x20, x6, lsl #3]",
    }
    for name, body in cases.items():
        k.kernel(f"l_{name}", body, setup="  mov x5, x20\n  str x20, [x20]", note=f"chain: {name}")
    # Known answers for the raw events: a load or a store to a new line of 128 bytes each pass, over
    # more than the L1's 128 KiB.
    k.kernel("v_l1miss_ld", "  add x5, x5, #128\n  and x5, x5, #0x3fffff\n  ldr x6, [x20, x5]", setup="  mov x5, #0", note="a load per line over 4 MiB")
    k.kernel("v_l1miss_st", "  add x5, x5, #128\n  and x5, x5, #0x3fffff\n  str x6, [x20, x5]", setup="  mov x5, #0", note="a store per line over 4 MiB")
    # A store's data and its address from a load: how long until the store can complete does not
    # show in a loop; a branch on a load shows nothing either. A misprediction's cost does.
    for st, what in ((0, "one class"), (2, "random bit")):
        k.kernel(f"fe_mis_{st}", f"""  ldrh w5, [x25, x24, lsl #1]
  add x24, x24, #1
  and x24, x24, #0xffff
  and w6, w5, #32
  cbz w6, 3f
  nop
3:""", setup=f"  mov x25, #({st + 1} << 18)\n  add x25, x22, x25\n  mov x24, #0", note=f"a branch on a bit from a table: {what}")


def build(k):
    ports(k)
    widths(k)
    latencies(k)
    fusion(k)
