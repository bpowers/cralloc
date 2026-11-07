#!/usr/bin/env python3
"""Rewrites the inlined routines of a scratch copy of the repository into a variant, to ask whether
real jobs feel the store-to-load hop on the hot word: variant.py COPY VARIANT

  plain    the routines as they are
  chainK   K more cycles on the hop: K ALU instructions between the new word and its commit, on the
           word's chain, and nothing else waits for them (K even)
  freeK    the same instructions, in the same bytes, on another register, off every chain: their
           cost as work, so the difference from chainK is what the chain alone costs
  kept     the slab's address kept in x18 and compared on entry: the load of a constant class's word
           renames, so its hop is free. A run-time class stays plain. Needs RUSTC_BOOTSTRAP=1, since
           only `is_val_statically_known` can tell a constant class on the way to an immediate.

Writes `crates/cralloc/src/front/provisional/aarch64_inline/slab.rs` and, for `kept`, `real.rs` and
`lib.rs` in COPY, from the copies it keeps beside them.
"""
import os
import re
import shutil
import sys

copy, variant = sys.argv[1], sys.argv[2]
crate = os.path.join(copy, "crates/cralloc/src")
files = {name: os.path.join(crate, name) for name in
         ("front/provisional/aarch64_inline/slab.rs", "front/real.rs", "lib.rs")}
text = {}
for name, path in files.items():
    if not os.path.exists(path + ".orig"):
        shutil.copy(path, path + ".orig")
    text[name] = open(path + ".orig").read()
inline = "front/provisional/aarch64_inline/slab.rs"


def alu(reg, kept, k):
    """K instructions that take a cycle each and do not fold: even K, adds and subtracts of 8."""
    lines = []
    for i in range(k):
        op = "add" if i % 2 == 0 else "sub"
        src = reg if kept else "x3"
        lines.append(f'            "{op} {reg}, {src}, #8",\n')
    return "".join(lines)


def sub(s, old, new):
    assert s.count(old) == 1, old
    return s.replace(old, new)


m = re.fullmatch(r"(chain|free)(\d+)", variant)
if m:
    kind, k = m.group(1), int(m.group(2))
    assert k % 2 == 0
    s = text[inline]
    on_chain = kind == "chain"
    pop_reg, push_reg = ("x1", "x9") if on_chain else ("x12", "x12")
    s = sub(s, '            "sub x1, x1, #8",\n            "str x1, [x3, x8]",\n',
            '            "sub x1, x1, #8",\n' + alu(pop_reg, on_chain, k) + '            "str x1, [x3, x8]",\n')
    s = sub(s, '            "mov x4, x9",\n            "str x9, [x3, x8]",\n',
            '            "mov x4, x9",\n' + alu(push_reg, on_chain, k) + '            "str x9, [x3, x8]",\n')
    s = s.replace('            out("x11") _,\n', '            out("x11") _,\n            out("x12") _,\n')
    text[inline] = s
elif variant == "kept":
    positions = " ".join(str(4 * c + 3) for c in range(1, 45))
    routines = """
#[inline(always)]
pub(crate) unsafe fn kept_pop(position: usize, word: usize, mask: usize) -> usize {
    if !core::intrinsics::is_val_statically_known(position) {
        return unsafe { slab_pop(word, mask) };
    }
    let ret: usize;
    macro_rules! arms {
        ($($p:literal)*) => {
            match position {
                $($p => unsafe {
                    core::arch::asm!(
                        a64_arm_direct!(),
                        "mov x0, #0",
                        "ldr w8, [x10, #4]",
                        "adrp x9, {SLABS}",
                        "ubfiz x8, x8, #12, #13",
                        "add x9, x9, x8",
                        "cmp x18, x9",
                        "b.ne 20f",
                        "21:",
                        "ldr x1, [x18, #{w}]",
                        "tst w1, w2",
                        "b.eq 7f",
                        "ldr x0, [x1, #-8]",
                        "sub x1, x1, #8",
                        "str x1, [x18, #{w}]",
                        a64_end_direct!(),
                        ".subsection 1",
                        "20:",
                        "mov x18, x9",
                        "b 21b",
                        ".subsection 0",
                        SLABS = sym crate::front::SLABS,
                        w = const $p * 8,
                        in("x2") mask,
                        out("x0") ret,
                        out("x1") _,
                        out("x8") _,
                        out("x9") _,
                        out("x10") _,
                        out("x11") _,
                        out("x18") _,
                        options(nostack),
                    )
                },)*
                _ => unsafe { core::hint::unreachable_unchecked() },
            }
        };
    }
    arms!(POSITIONS);
    ret
}

#[inline(always)]
pub(crate) unsafe fn kept_push(position: usize, word: usize, mask: usize, p: usize) -> bool {
    if !core::intrinsics::is_val_statically_known(position) {
        return unsafe { slab_push(word, mask, p) };
    }
    let pushed: usize;
    macro_rules! arms {
        ($($p:literal)*) => {
            match position {
                $($p => unsafe {
                    core::arch::asm!(
                        a64_arm_direct!(),
                        "mov x4, #0",
                        "ldr w8, [x10, #4]",
                        "adrp x9, {SLABS}",
                        "ubfiz x8, x8, #12, #13",
                        "add x9, x9, x8",
                        "cmp x18, x9",
                        "b.ne 20f",
                        "21:",
                        "ldr x1, [x18, #{w}]",
                        "add x9, x1, #8",
                        "tst w9, w2",
                        "b.eq 7f",
                        "str x0, [x1]",
                        "mov x4, x9",
                        "str x9, [x18, #{w}]",
                        a64_end_direct!(),
                        ".subsection 1",
                        "20:",
                        "mov x18, x9",
                        "b 21b",
                        ".subsection 0",
                        SLABS = sym crate::front::SLABS,
                        w = const $p * 8,
                        in("x0") p,
                        in("x2") mask,
                        out("x4") pushed,
                        out("x1") _,
                        out("x8") _,
                        out("x9") _,
                        out("x10") _,
                        out("x11") _,
                        out("x18") _,
                        options(nostack),
                    )
                },)*
                _ => unsafe { core::hint::unreachable_unchecked() },
            }
        };
    }
    arms!(POSITIONS);
    pushed != 0
}
""".replace("POSITIONS", positions)
    text[inline] += routines
    s = text["front/real.rs"]
    s = sub(s, "inline::slab_pop(SLABS.word(class), class.mask())",
            "inline::kept_pop(class.word(), SLABS.word(class), class.mask())")
    s = sub(s, "inline::slab_push(SLABS.word(class), class.mask(), object)",
            "inline::kept_push(class.word(), SLABS.word(class), class.mask(), object)")
    text["front/real.rs"] = s
    s = text["lib.rs"]
    first = s.index("#![")
    text["lib.rs"] = s[:first] + "#![feature(core_intrinsics)]\n#![allow(internal_features)]\n" + s[first:]
elif variant != "plain":
    sys.exit(f"unknown variant {variant}")

for name, path in files.items():
    open(path, "w").write(text[name])
