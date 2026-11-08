"""Store-to-load forwarding, atomics and barriers."""


def chain(load, store, add="  add x5, x5, #1"):
    return f"{load}\n{add}\n{store}"


def forwarding(k):
    # A word in the state block, and the same address through other registers.
    regs = "  add x6, x20, #16\n  mov x7, x6\n  mov x10, #16\n  mov x11, #2\n  add x12, x20, #8\n"
    cases = [
        ("x_imm", "  ldr x5, [x20, #16]", "  str x5, [x20, #16]", "64-bit [x, #16]"),
        ("x_base", "  ldr x5, [x6]", "  str x5, [x6]", "64-bit [x]"),
        ("x_reg", "  ldr x5, [x20, x10]", "  str x5, [x20, x10]", "64-bit [x, x]"),
        ("x_regsh", "  ldr x5, [x20, x11, lsl #3]", "  str x5, [x20, x11, lsl #3]", "64-bit [x, x, lsl #3]"),
        ("x_uxtw", "  ldr x5, [x20, w10, uxtw]", "  str x5, [x20, w10, uxtw]", "64-bit [x, w, uxtw]"),
        ("w_imm", "  ldr w5, [x20, #16]", "  str w5, [x20, #16]", "32-bit [x, #16]"),
        ("w_base", "  ldr w5, [x6]", "  str w5, [x6]", "32-bit [x]"),
        ("w_reg", "  ldr w5, [x20, x10]", "  str w5, [x20, x10]", "32-bit [x, x]"),
        ("h_imm", "  ldrh w5, [x20, #16]", "  strh w5, [x20, #16]", "16-bit [x, #16]"),
        ("h_reg", "  ldrh w5, [x20, x10]", "  strh w5, [x20, x10]", "16-bit [x, x]"),
        ("b_imm", "  ldrb w5, [x20, #16]", "  strb w5, [x20, #16]", "8-bit [x, #16]"),
        ("b_reg", "  ldrb w5, [x20, x10]", "  strb w5, [x20, x10]", "8-bit [x, x]"),
        ("x_diffimm", "  ldr x5, [x6]", "  str x5, [x12, #8]", "64-bit, load and store through different registers"),
        ("x_alias", "  ldr x5, [x6]", "  str x5, [x7]", "64-bit, different registers, same offset form"),
        ("x_ldimm_streg", "  ldr x5, [x20, #16]", "  str x5, [x20, x10]", "64-bit, load [x, #16], store [x, x]"),
        ("x_ldreg_stimm", "  ldr x5, [x20, x10]", "  str x5, [x20, #16]", "64-bit, load [x, x], store [x, #16]"),
    ]
    for name, load, store, note in cases:
        k.kernel(f"f_{name}", chain(load, store), setup=regs, note="load; add; store: " + note)

    # The base register changes every pass.
    k.kernel("f_x_movbase", "  mov x8, x6\n" + chain("  ldr x5, [x8]", "  str x5, [x8]"), setup=regs, note="base copied each pass")
    k.kernel("f_x_addbase", "  add x8, x20, #16\n" + chain("  ldr x5, [x8]", "  str x5, [x8]"), setup=regs, note="base recomputed by add each pass")
    k.kernel("f_x_addregbase", "  add x8, x20, x10\n" + chain("  ldr x5, [x8]", "  str x5, [x8]"), setup=regs, note="base recomputed by add reg each pass")
    k.kernel("f_x_ldbase", "  ldr x8, [x20, #256]\n" + chain("  ldr x5, [x8]", "  str x5, [x8]"),
             setup=regs + "  str x6, [x20, #256]", note="base reloaded from memory each pass")
    k.kernel("f_x_adrp", "  adrp x8, rf_data\n" + chain("  ldr x5, [x8, :lo12:rf_data]", "  str x5, [x8, :lo12:rf_data]"), note="adrp each pass, :lo12:")
    k.kernel("f_x_sp", chain("  ldr x5, [sp, #16]", "  str x5, [sp, #16]"), setup="  sub sp, sp, #64", fini="  add sp, sp, #64", note="64-bit [sp, #16]")
    k.kernel("f_x_tp", "  mrs x8, tpidr_el0\n" + chain("  ldr x5, [x8, x21]", "  str x5, [x8, x21]"),
             setup="  sub x21, x20, x20\n  mrs x8, tpidr_el0\n  sub x21, x20, x8\n  add x21, x21, #16", note="thread pointer + register each pass")

    # Writeback: the address register is updated by the access.
    k.kernel("f_x_pre", "  ldr x5, [x6, #8]!\n  add x5, x5, #1\n  str x5, [x6], #-8", setup=regs, note="load pre-index, store post-index, same word")
    k.kernel("f_x_prepost", "  ldr x5, [x6], #-8\n  add x5, x5, #1\n  str x5, [x6, #8]!", setup=regs, note="load post-index, store pre-index, same word")

    # A chain of k adds between the load and the store: slope 1 means the hop is constant.
    for n in (1, 2, 3, 4, 5, 6, 8, 12, 16):
        adds = "\n".join("  add x5, x5, #1" for _ in range(n))
        k.kernel(f"f_adds{n}", f"  ldr x5, [x20, #16]\n{adds}\n  str x5, [x20, #16]", note=f"load; {n} adds; store")
    k.kernel("f_noadd", "  ldr x5, [x20, #16]\n  str x5, [x20, #16]", note="load; store the same register")
    k.kernel("f_noadd_mov", "  ldr x5, [x20, #16]\n  mov x6, x5\n  str x6, [x20, #16]", note="load; mov; store")
    k.kernel("f_st_ld_add", "  str x5, [x20, #16]\n  ldr x5, [x20, #16]\n  add x5, x5, #1", note="store first: store; load; add")
    k.kernel("f_st_ld_only", "  str x5, [x20, #16]\n  ldr x5, [x20, #16]", note="store; load: the bare hop")
    k.kernel("f_ld_only", "  ldr x5, [x20, #16]", note="load alone, no store")
    k.kernel("f_ld_add_nost", "  ldr x5, [x20, #16]\n  add x5, x5, #1", note="load; add, no store")
    k.kernel("f_st_only", "  add x5, x5, #1\n  str x5, [x20, #16]", note="add; store, no load")

    # Time between store and load: nops, unrelated stores, unrelated loads.
    for n in (1, 2, 4, 8, 16, 24, 32, 48, 64, 96, 128, 192):
        nops = "\n".join("  nop" for _ in range(n))
        k.kernel(f"f_gapnop{n}", chain("  ldr x5, [x20, #16]", "  str x5, [x20, #16]") + "\n" + nops, note=f"hop with {n} nops after the store")
    for n in (1, 2, 4, 8, 16, 32, 64):
        stores = "\n".join(f"  str x6, [x20, #{256 + 8 * i}]" for i in range(n))
        k.kernel(f"f_gapst{n}", chain("  ldr x5, [x20, #16]", "  str x5, [x20, #16]") + "\n" + stores, note=f"hop with {n} unrelated stores after")
    for n in (1, 2, 4, 8, 16):
        loads = "\n".join(f"  ldr x6, [x20, #{256 + 8 * i}]" for i in range(n))
        k.kernel(f"f_gapld{n}", chain("  ldr x5, [x20, #16]", "  str x5, [x20, #16]") + "\n" + loads, note=f"hop with {n} unrelated loads after")
    # Several independent chains: how many hops a cycle.
    for n in (2, 3, 4, 6):
        body = "\n".join(chain(f"  ldr x{5 + i}, [x20, #{64 * i + 16}]", f"  str x{5 + i}, [x20, #{64 * i + 16}]", f"  add x{5 + i}, x{5 + i}, #1") for i in range(n))
        k.kernel(f"f_chains{n}", body, note=f"{n} independent load; add; store chains")
    # Same word, two hops a pass.
    k.kernel("f_two_hops", chain("  ldr x5, [x20, #16]", "  str x5, [x20, #16]") + "\n" + chain("  ldr x5, [x20, #16]", "  str x5, [x20, #16]", "  sub x5, x5, #1"),
             note="2 hops a pass on one word, as a pair")
    k.kernel("f_two_hops_diff", chain("  ldr x5, [x20, #16]", "  str x5, [x20, #16]") + "\n" + chain("  ldr x6, [x20, #24]", "  str x6, [x20, #24]", "  sub x6, x6, #1"),
             note="2 hops a pass on two words, 1 each: a chain each")

    # Partial overlaps.
    k.kernel("f_s64_l32", "  str x5, [x20, #16]\n  ldr w5, [x20, #16]\n  add x5, x5, #1", note="store 64, load 32 low")
    k.kernel("f_s64_l32hi", "  str x5, [x20, #16]\n  ldr w5, [x20, #20]\n  add x5, x5, #1", note="store 64, load 32 high")
    k.kernel("f_s64_l16", "  str x5, [x20, #16]\n  ldrh w5, [x20, #16]\n  add x5, x5, #1", note="store 64, load 16 low")
    k.kernel("f_s64_l8", "  str x5, [x20, #16]\n  ldrb w5, [x20, #16]\n  add x5, x5, #1", note="store 64, load 8 low")
    k.kernel("f_s32_l16", "  str w5, [x20, #16]\n  ldrh w5, [x20, #16]\n  add x5, x5, #1", note="store 32, load 16 low")
    k.kernel("f_s32_l64", "  str w5, [x20, #16]\n  ldr x5, [x20, #16]\n  add x5, x5, #1", note="store 32, load 64: not covered")
    k.kernel("f_s8_l64", "  strb w5, [x20, #16]\n  ldr x5, [x20, #16]\n  add x5, x5, #1", note="store 8, load 64: not covered")
    k.kernel("f_stp_ldr1", "  stp x5, x6, [x20, #16]\n  ldr x5, [x20, #16]\n  add x5, x5, #1", note="stp, load its first half")
    k.kernel("f_stp_ldr2", "  stp x6, x5, [x20, #16]\n  ldr x5, [x20, #24]\n  add x5, x5, #1", note="stp, load its second half")
    k.kernel("f_stp_ldp", "  stp x5, x6, [x20, #16]\n  ldp x5, x6, [x20, #16]\n  add x5, x5, #1", note="stp then ldp")
    k.kernel("f_str2_ldp", "  str x5, [x20, #16]\n  str x6, [x20, #24]\n  ldp x5, x6, [x20, #16]\n  add x5, x5, #1", note="2 str then ldp over both")
    k.kernel("f_str_ldp_half", "  str x5, [x20, #16]\n  ldp x5, x6, [x20, #16]\n  add x5, x5, #1", note="1 str then ldp over it and its neighbour")
    k.kernel("f_w_w_x", "  str w5, [x20, #16]\n  str w6, [x20, #20]\n  ldr x5, [x20, #16]\n  add x5, x5, #1", note="2 str w then ldr x over both")

    # The word's position relative to lines: a word across 8 or 16 bytes.
    k.kernel("f_x_unal4", chain("  ldr x5, [x20, #20]", "  str x5, [x20, #20]"), note="64-bit at 20: misaligned")
    k.kernel("f_x_unal60", chain("  ldur x5, [x20, #60]", "  stur x5, [x20, #60]"), note="64-bit across a 64-byte boundary")
    k.kernel("f_x_unal124", chain("  ldur x5, [x20, #124]", "  stur x5, [x20, #124]"), note="64-bit across a 128-byte boundary")

    # Whether a load waits on a store whose address comes late, and what a wrong guess costs.
    k.kernel("f_late_same", "  ldr x8, [x6]\n  str x7, [x8]\n  ldr x9, [x8]",
             setup=regs + "  add x8, x20, #128\n  str x8, [x6]", note="store through a loaded address, load the same")
    k.kernel("f_late_other", "  ldr x8, [x6]\n  str x7, [x8]\n  ldr x9, [x20, #1024]",
             setup=regs + "  add x8, x20, #128\n  str x8, [x6]", note="store through a loaded address, load another")
    k.kernel("f_late_chain_other", "  ldr x8, [x6]\n  ldr x8, [x8]\n  ldr x8, [x8]\n  ldr x8, [x8]\n  str x7, [x8, #8]\n  ldr x9, [x20, #1024]",
             setup=regs + "  add x8, x20, #128\n  str x8, [x6]\n  str x8, [x8]", note="store at the end of a pointer chain, load another")
    # A store-to-load dependence that holds only sometimes.
    for period in (2, 4, 8, 16, 32, 64):
        mask = period * 8 - 8
        k.kernel(f"f_sometimes{period}", f"""  add x8, x8, #8
  and x8, x8, #{mask}
  add x9, x20, x8
  str x8, [x9, #2048]
  ldr x10, [x20, #2048]
  add x11, x11, x10""",
                 setup="  mov x8, #0\n  mov x11, #0", note=f"store to one of {period} words, load the first; aliased 1 pass in {period}")
    # The same, with the loaded value feeding the next store address: a chain only when aliased.
    for period in (2, 4, 8, 16, 32, 64):
        mask = period * 8 - 8
        k.kernel(f"f_chain_sometimes{period}", f"""  ldr x10, [x20, #2048]
  add x8, x8, #8
  and x8, x8, #{mask}
  add x9, x20, x8
  add x10, x10, #1
  str x10, [x9, #2048]""",
                 setup="  mov x8, #0", note=f"load; add; store to one of {period} words: chain 1 pass in {period}")


def renaming(k):
    """When does a load take the value of the store before it at no cost? Each cell is one change
    to a loop that runs at 1 cycle: same base register, not written, immediate offset, 64 bits."""
    regs = "  add x6, x20, #16\n  mov x8, x6\n  mov x10, #16\n"
    one = chain("  ldr x5, [x20, #16]", "  str x5, [x20, #16]")
    k.kernel("r_base", one, setup=regs, note="[x, #16], base never written")
    k.kernel("r_base2", chain("  ldr x5, [x6]", "  str x5, [x6]"), setup=regs, note="[x], base never written")
    # What writes to the base stop it?
    via = chain("  ldr x5, [x8]", "  str x5, [x8]")
    k.kernel("r_wr_mov", "  mov x8, x6\n" + via, setup=regs, note="base written by mov before the load")
    k.kernel("r_wr_self", "  mov x8, x8\n" + via, setup=regs, note="base moved to itself")
    k.kernel("r_wr_add0", "  add x8, x6, #0\n" + via, setup=regs, note="base written by add #0")
    k.kernel("r_wr_add", "  add x8, x20, #16\n" + via, setup=regs, note="base written by add")
    k.kernel("r_wr_mid", "  ldr x5, [x8]\n  mov x8, x6\n  add x5, x5, #1\n  str x5, [x8]", setup=regs, note="base written between load and store, not store and load")
    k.kernel("r_wr_mid_add", "  ldr x5, [x8]\n  add x8, x20, #16\n  add x5, x5, #1\n  str x5, [x8]", setup=regs, note="same, base written by add")
    k.kernel("r_wr_mid_two", "  ldr x5, [x8]\n  add x5, x5, #1\n  add x8, x20, #16\n  str x5, [x8]\n  nop", setup=regs, note="same, write just before the store")
    k.kernel("r_sp", chain("  ldr x5, [sp, #16]", "  str x5, [sp, #16]"), setup="  sub sp, sp, #64", fini="  add sp, sp, #64", note="[sp, #16], sp unchanged")
    k.kernel("r_sp_move", "  sub sp, sp, #16\n  add sp, sp, #16\n" + chain("  ldr x5, [sp, #16]", "  str x5, [sp, #16]"), setup="  sub sp, sp, #64", fini="  add sp, sp, #64", note="[sp, #16], sp moved and back each pass")
    k.kernel("r_data_keep", "  ldr x5, [x20, #16]\n  add x7, x5, #1\n  str x7, [x20, #16]", setup=regs, note="other register for the new value")
    k.kernel("r_xzr", "  str xzr, [x20, #16]\n  ldr x5, [x20, #16]\n  add x7, x5, #1", setup=regs, note="store zero; load; add: no chain through memory")
    k.kernel("r_ldur", chain("  ldur x5, [x20, #-16]", "  stur x5, [x20, #-16]"), setup="  add x20, x20, #128", note="ldur/stur with a negative offset")
    k.kernel("r_off_big", chain("  ldr x5, [x20, #32000]", "  str x5, [x20, #32000]"), note="largest scaled offset form")
    # Two hops on one word, as a pair: store-load adjacency.
    for gap in (0, 1, 2, 3, 4, 5, 6, 7, 8, 12, 16, 24):
        nops = "\n".join("  nop" for _ in range(gap))
        k.kernel(f"r_two_gap{gap}", chain("  ldr x5, [x20, #16]", "  str x5, [x20, #16]") + f"\n{nops}\n" + chain("  ldr x6, [x20, #16]", "  str x6, [x20, #16]", "  sub x6, x6, #1"),
                 note=f"2 hops on one word, {gap} nops between first store and second load")
    for gap in (0, 2, 4, 8, 12, 16, 24):
        nops = "\n".join("  nop" for _ in range(gap))
        k.kernel(f"r_two_both{gap}", chain("  ldr x5, [x20, #16]", "  str x5, [x20, #16]") + f"\n{nops}\n" + chain("  ldr x5, [x20, #16]", "  str x5, [x20, #16]", "  sub x5, x5, #1") + f"\n{nops}",
                 note=f"2 hops on one word, {gap} nops after each store")
    k.kernel("r_two_branch", chain("  ldr x5, [x20, #16]", "  str x5, [x20, #16]") + "\n  b 3f\n3:\n" + chain("  ldr x5, [x20, #16]", "  str x5, [x20, #16]", "  sub x5, x5, #1"),
             note="2 hops on one word, a taken branch between")
    # A bare hop through forms that cannot be renamed, by the chain between: the forwarding curve.
    for n in (0, 1, 2, 3, 4, 5, 6, 7, 8, 10, 12, 16, 24):
        adds = "\n".join("  add x5, x5, #1" for _ in range(n))
        k.kernel(f"h_reg{n}", f"  ldr x5, [x20, x10]\n{adds}\n  str x5, [x20, x10]", setup=regs, note=f"[x, x]: load; {n} adds; store")
        k.kernel(f"h_w{n}", f"  ldr w5, [x20, #16]\n" + "\n".join("  add w5, w5, #1" for _ in range(n)) + "\n  str w5, [x20, #16]", setup=regs, note=f"32-bit: load; {n} adds; store")
    # Whether a narrow hop renames when its add is of the same width as the load.
    k.kernel("h_h1", "  ldrh w5, [x20, #16]\n  add w5, w5, #1\n  strh w5, [x20, #16]", setup=regs, note="16-bit, add w: load; add; store")
    k.kernel("h_b1", "  ldrb w5, [x20, #16]\n  add w5, w5, #1\n  strb w5, [x20, #16]", setup=regs, note="8-bit, add w: load; add; store")
    k.kernel("h_x_w1", "  ldr x5, [x20, #16]\n  add x5, x5, #1\n  str x5, [x20, #16]", setup=regs, note="64-bit, add x, beside the two above")
    # The recomputed base, which every per-CPU word has.
    for n in (0, 1, 2, 3, 4, 5, 6, 8, 12, 16):
        adds = "\n".join("  add x5, x5, #1" for _ in range(n))
        k.kernel(f"h_add{n}", f"  add x8, x20, x10\n  ldr x5, [x8]\n{adds}\n  str x5, [x8]", setup=regs, note=f"recomputed base: load; {n} adds; store")
    # Eligibility against the loop's length: a renamed hop runs at max(K, length / 8).
    for adds_n in (1, 2, 3, 4, 5, 6, 7):
        for fill in range(0, 13):
            adds = "\n".join("  add x5, x5, #1" for _ in range(adds_n))
            nops = "\n".join("  nop" for _ in range(fill))
            k.kernel(f"rk_{adds_n}_{fill}", f"  ldr x5, [x20, #16]\n{adds}\n  str x5, [x20, #16]\n{nops}", setup=regs, note=f"[x, #16]: load; {adds_n} adds; store; {fill} nops")
    # The hop's other half: store first.
    k.kernel("h_str_ldr", "  str x5, [x20, x10]\n  ldr x5, [x20, x10]", setup=regs, note="[x, x]: store; load")
    k.kernel("h_str_ldr_add", "  str x5, [x20, x10]\n  ldr x5, [x20, x10]\n  add x5, x5, #1", setup=regs, note="[x, x]: store; load; add")
    # Data dependent on a load result as store data, no ALU: does it avoid the load-to-ALU cycle?
    k.kernel("h_two_nop", "  ldr x5, [x20, x10]\n  str x5, [x20, x10]\n  ldr x6, [x20, x10]\n  str x6, [x20, x10]", setup=regs, note="[x, x]: 2 hops, no add")
    k.kernel("h_two_add", "  ldr x5, [x20, x10]\n  add x5, x5, #1\n  str x5, [x20, x10]\n  ldr x6, [x20, x10]\n  sub x6, x6, #1\n  str x6, [x20, x10]", setup=regs, note="[x, x]: 2 hops, an add each")
    # Pre-index writeback in place of the add: the chain from the load through an AGU.
    k.kernel("h_wb_pre", "  ldr x5, [x20, x10]\n  ldr x7, [x5, #-8]!\n  str x5, [x20, x10]\n  ldr x5, [x20, x10]\n  ldr x7, [x5, #8]!\n  str x5, [x20, x10]",
             setup=regs + "  str x20, [x20, #16]", note="[x, x]: 2 hops, pre-index writeback for each add")
    k.kernel("h_wb_post", "  ldr x5, [x20, x10]\n  ldr x7, [x5], #-8\n  str x5, [x20, x10]\n  ldr x5, [x20, x10]\n  ldr x7, [x5], #8\n  str x5, [x20, x10]",
             setup=regs + "  str x20, [x20, #16]", note="[x, x]: 2 hops, post-index writeback for each add")
    k.kernel("h_wb_st", "  ldr x5, [x20, x10]\n  str x7, [x5, #-8]!\n  str x5, [x20, x10]\n  ldr x5, [x20, x10]\n  str x7, [x5, #8]!\n  str x5, [x20, x10]",
             setup=regs + "  str x20, [x20, #16]\n  add x7, x20, #256", note="[x, x]: 2 hops, store pre-index writeback for each add")


def atomics(k):
    obj = "  add x6, x20, #256\n  mov x7, #1\n  mov x8, #0\n  add x9, x20, #1024\n"
    work = "\n".join("  add x10, x10, #1" for _ in range(16))
    chain4 = "\n".join("  ldr x11, [x11]" for _ in range(4))
    ptr = "  add x11, x20, #2048\n  str x11, [x11]\n"
    ops = {
        "ldadd": ("  ldadd x7, x8, [x6]", "ldadd"),
        "ldadda": ("  ldadda x7, x8, [x6]", "ldadda"),
        "ldaddl": ("  ldaddl x7, x8, [x6]", "ldaddl"),
        "ldaddal": ("  ldaddal x7, x8, [x6]", "ldaddal"),
        "stadd": ("  ldadd x7, xzr, [x6]", "stadd: no result"),
        "swp": ("  swp x7, x8, [x6]", "swp"),
        "swpal": ("  swpal x7, x8, [x6]", "swpal"),
        "cas": ("  ldr x8, [x6]\n  add x12, x8, #1\n  cas x8, x12, [x6]", "load; cas that succeeds"),
        "casa": ("  ldr x8, [x6]\n  add x12, x8, #1\n  casa x8, x12, [x6]", "load; casa"),
        "casl": ("  ldr x8, [x6]\n  add x12, x8, #1\n  casl x8, x12, [x6]", "load; casl"),
        "casal": ("  ldr x8, [x6]\n  add x12, x8, #1\n  casal x8, x12, [x6]", "load; casal"),
        "casfail": ("  mov x8, #5\n  cas x8, x12, [x6]", "cas that fails"),
        "casalfail": ("  mov x8, #5\n  casal x8, x12, [x6]", "casal that fails"),
        "llsc": ("3:\n  ldxr x8, [x6]\n  add x8, x8, #1\n  stxr w12, x8, [x6]\n  cbnz w12, 3b", "ldxr; add; stxr"),
        "llscal": ("3:\n  ldaxr x8, [x6]\n  add x8, x8, #1\n  stlxr w12, x8, [x6]\n  cbnz w12, 3b", "ldaxr; add; stlxr"),
        "ldar": ("  ldar x8, [x6]", "ldar"),
        "ldapr": ("  ldapr x8, [x6]", "ldapr"),
        "ldr": ("  ldr x8, [x6]", "plain ldr"),
        "stlr": ("  stlr x7, [x6]", "stlr"),
        "str": ("  str x7, [x6]", "plain str"),
        "dmb": ("  dmb ish", "dmb ish"),
        "ldsetal": ("  ldsetal x7, x8, [x6]", "ldsetal, as a bitmap word's OR"),
        "ldset": ("  ldset x7, x8, [x6]", "ldset"),
        "dmb_ldset_dmb": ("  dmb ish\n  ldset x7, x8, [x6]\n  dmb ish", "ldset between two dmb ish"),
        "dmb_cas_dmb": ("  dmb ish\n  ldr x8, [x6]\n  add x12, x8, #1\n  cas x8, x12, [x6]\n  dmb ish", "load, cas between two dmb ish"),
        "dmbst": ("  dmb ishst", "dmb ishst"),
        "dmbld": ("  dmb ishld", "dmb ishld"),
        "isb": ("  isb", "isb"),
    }
    for name, (body, note) in ops.items():
        k.kernel(f"a_{name}", body, setup=obj, note=note + " alone")
        k.kernel(f"a_{name}_work", f"{body}\n{work}", setup=obj, note=note + " beside 16 unrelated dependent adds")
        k.kernel(f"a_{name}_chain", f"{body}\n{chain4}", setup=obj + ptr, note=note + " beside 4 unrelated chained loads")
        k.kernel(f"a_{name}_st", f"{body}\n  str x7, [x9]", setup=obj, note=note + " then an unrelated store")
        k.kernel(f"a_{name}_ld", f"{body}\n  ldr x13, [x9]", setup=obj, note=note + " then an unrelated load")
    k.kernel("a_work", work, setup=obj, note="16 dependent adds alone")
    k.kernel("a_chain", chain4, setup=obj + ptr, note="4 chained loads alone")
    k.kernel("a_ldadd2", "  ldadd x7, x8, [x6]\n  ldadd x7, x12, [x9]", setup=obj, note="2 ldadd, different lines")
    k.kernel("a_ldaddal2", "  ldaddal x7, x8, [x6]\n  ldaddal x7, x12, [x9]", setup=obj, note="2 ldaddal, different lines")
    k.kernel("a_ldapr_chain_dep", "  ldapr x11, [x11]\n  ldapr x11, [x11]", ops=2, setup=obj + ptr, note="ldapr pointer chain, per load")
    k.kernel("a_ldar_chain_dep", "  ldar x11, [x11]\n  ldar x11, [x11]", ops=2, setup=obj + ptr, note="ldar pointer chain, per load")
    k.kernel("a_stlr_ld_fwd", "  stlr x7, [x6]\n  ldr x7, [x6]\n  add x7, x7, #1", setup=obj, note="stlr then plain load of it: hop")
    k.kernel("a_str_ldapr_fwd", "  str x7, [x6]\n  ldapr x7, [x6]\n  add x7, x7, #1", setup=obj, note="str then ldapr of it: hop")
    k.kernel("a_stlr_ldapr_fwd", "  stlr x7, [x6]\n  ldapr x7, [x6]\n  add x7, x7, #1", setup=obj, note="stlr then ldapr of it: hop")
    k.kernel("a_stlr_ldar_fwd", "  stlr x7, [x6]\n  ldar x7, [x6]\n  add x7, x7, #1", setup=obj, note="stlr then ldar of it: hop")

    # A lock-free stack's pop then push of one node: head at x6, the node holds the next link.
    stack = obj + "  add x13, x20, #512\n  str x13, [x6]\n  str xzr, [x13]\n"
    for name, cas_pop, cas_push in (("cas", "cas", "cas"), ("casa_l", "casa", "casl"), ("casal", "casal", "casal")):
        body = f"""  ldr x8, [x6]
  ldr x12, [x8]
  mov x14, x8
  {cas_pop} x14, x12, [x6]
  cmp x14, x8
  b.ne 9f
  ldr x14, [x6]
  str x14, [x8]
  mov x15, x14
  {cas_push} x15, x8, [x6]
  cmp x15, x14
  b.ne 9f"""
        k.kernel(f"a_stack_{name}", body, setup=stack, note=f"lock-free stack pop then push, {cas_pop} and {cas_push}")
    llsc = """3:
  ldaxr x8, [x6]
  ldr x12, [x8]
  stlxr w14, x12, [x6]
  cbnz w14, 3b
4:
  ldaxr x12, [x6]
  str x12, [x8]
  stlxr w14, x8, [x6]
  cbnz w14, 4b"""
    k.kernel("a_stack_llsc", llsc, setup=stack, note="lock-free stack pop then push, ldaxr/stlxr")
    # A Vyukov-style queue's push: a swap and a store.
    k.kernel("a_mpsc", "  mov x8, x6\n  swpal x8, x8, [x9]\n  str x6, [x8]", setup=obj + "  str x6, [x9]", note="swap then store, an MPSC push")


def build(k):
    store_bursts(k)
    forwarding(k)
    renaming(k)
    atomics(k)


def store_bursts(k):
    """A fill's loop: pointers written downward through a magazine's 8 KiB, by what each form of store
    sustains. The value's chain is kept off the loop: two pointers step two strides."""
    setup = "  mov x12, x22\n  add x13, x12, #24\n  mov x15, #48\n  dup v0.2d, x12\n  dup v1.2d, x13\n  dup v2.2d, x15\n"
    forms = {
        "str": ("""  str x12, [x14]
  stur x13, [x14, #-8]
  add x12, x12, x15
  add x13, x13, x15
  sub x14, x14, #16""", 2),
        "stp": ("""  stp x13, x12, [x14, #-8]
  add x12, x12, x15
  add x13, x13, x15
  sub x14, x14, #16""", 2),
        "stp2": ("""  stp x13, x12, [x14, #-8]
  add x16, x12, x15
  add x17, x13, x15
  stp x17, x16, [x14, #-24]
  add x12, x16, x15
  add x13, x17, x15
  sub x14, x14, #32""", 4),
        "stpq": ("""  stp q1, q0, [x14, #-16]
  add v0.2d, v0.2d, v2.2d
  add v1.2d, v1.2d, v2.2d
  sub x14, x14, #32""", 4),
    }
    for name, (body, step) in forms.items():
        k.kernel(f"st_burst_{name}", f"""  mov x14, #8184
  add x14, x22, x14
  mov x9, #(1024 / {step})
1:
{body}
  subs x9, x9, #1
  b.ne 1b""", setup=setup, ops=1024, note=f"8 KiB of pointers by {name}")
