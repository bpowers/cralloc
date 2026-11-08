#!/usr/bin/env python3
"""Rewrites the inlined routines of a scratch copy of the repository into a variant, to ask whether
real jobs feel the store-to-load hop on the hot word: variant.py COPY VARIANT

  plain    the routines as they are
  relaxed  the bitmap words' operations relaxed, to size what their order costs a single thread;
           wrong with more than one
  hugen    one binary whose huge-page switch is the environment's CRALLOC_HUGE_N, so that placement
           cancels; each value is an allocator of the copy, by `env` in its allocators.toml
  huge1    a class's growth switches to huge pages after one, not eight: the same bytes as on x86-64,
           where a kernel's huge pages are 32 MiB
  nostretch a launder and a fill with no dense path, to compare with them over placements
  area     the thread's rseq area read by a block the compiler may share and hoist, so a routine
           does not reload it: fewer instructions and two loads off the way to the pointer
  chainK   K more cycles on the hop: K ALU instructions between the new word and its commit, on the
           word's chain, and nothing else waits for them (K even)
  freeK    the same instructions, in the same bytes, on another register, off every chain: their
           cost as work, so the difference from chainK is what the chain alone costs
  kept     the slab's address kept in x18 and compared on entry: the load of a constant class's word
           renames, so its hop is free. A run-time class stays plain. Needs RUSTC_BOOTSTRAP=1, since
           only `is_val_statically_known` can tell a constant class on the way to an immediate. The
           write of x18 sits inside the sequence, so that a preemption there restarts it.

Writes `crates/cralloc/src/front/generated/aarch64_inline/slab.rs` and, for `kept`, `real.rs` and
`lib.rs` in COPY, from the copies it keeps beside them. That file holds each routine's two forms,
and COPY builds the one for glibc: a change to the text of both does not matter to the other.
"""
import os
import re
import shutil
import sys

copy, variant = sys.argv[1], sys.argv[2]
crate = os.path.join(copy, "crates/cralloc/src")
files = {name: os.path.join(crate, name) for name in
         ("front/generated/aarch64_inline/slab.rs", "front/real.rs", "lib.rs", "back/word.rs",
          "back/class.rs", "back/policy.rs", "back/region.rs", "environment.rs")}
text = {}
for name, path in files.items():
    if not os.path.exists(path + ".orig"):
        shutil.copy(path, path + ".orig")
    text[name] = open(path + ".orig").read()
inline = "front/generated/aarch64_inline/slab.rs"


def alu(reg, kept, k):
    """K instructions that take a cycle each and do not fold: even K, adds and subtracts of 8."""
    lines = []
    for i in range(k):
        op = "add" if i % 2 == 0 else "sub"
        src = reg if kept else "x3"
        lines.append(f"{op} {reg}, {src}, #8\\n")
    return "".join(lines)


def sub(s, old, new, n=1):
    assert s.count(old) == n, old
    return s.replace(old, new)


# A routine's text around its sequence, in the form for glibc.
ARM = r"""
                        "adrp x11, 3f",
                        "add x11, x11, :lo12:3f",
                        "ldr x9, [x11, #32]",
                        "ldr x9, [x9]",
                        "mrs x10, tpidr_el0",
                        "add x10, x10, x9",
                        "5:",
                        "str x11, [x10, #8]",
                        "6:","""
END = r"""
                        "7:",
                        ".pushsection __rseq_cs, \"aw\"",
                        ".balign 32",
                        "3:",
                        ".long 0, 0",
                        ".quad 6b, 7b - 6b, 4f",
                        ".quad __rseq_offset",
                        ".popsection",
                        ".subsection 1",
                        ".inst 0xd428bc00",
                        "4:",
                        "b 5b",
                        ".subsection 0","""

AREA = r"""
#[inline(always)]
fn area() -> usize {
    let area: usize;
    // SAFETY: reads the thread pointer and a variable that does not change after startup.
    unsafe {
        core::arch::asm!(
            ".pushsection .data.rel.ro.cralloc_got,\"aw\",%progbits",
            ".balign 16",
            "8:",
            ".quad __rseq_offset",
            ".popsection",
            "adrp {t}, 8b",
            "ldr {t}, [{t}, :lo12:8b]",
            "ldr {t}, [{t}]",
            "mrs {a}, tpidr_el0",
            "add {a}, {a}, {t}",
            a = out(reg) area,
            t = out(reg) _,
            options(pure, nomem, nostack, preserves_flags),
        );
    }
    area
}
"""

m = re.fullmatch(r"(chain|free)(\d+)", variant)
if m:
    kind, k = m.group(1), int(m.group(2))
    assert k % 2 == 0
    s = text[inline]
    on_chain = kind == "chain"
    pop_reg, push_reg = ("x1", "x9") if on_chain else ("x12", "x12")
    s = sub(s, r"sub x1, x1, #8\nstr x1, [x3, x8]\n",
            r"sub x1, x1, #8\n" + alu(pop_reg, on_chain, k) + r"str x1, [x3, x8]\n", 2)
    s = sub(s, r"mov x4, x9\nstr x9, [x3, x8]\n",
            r"mov x4, x9\n" + alu(push_reg, on_chain, k) + r"str x9, [x3, x8]\n", 2)
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
                        "adrp x12, {SLABS}",
                        ARM_TEXT
                        "mov x0, #0",
                        "ldr w8, [x10, #4]",
                        "cmp w8, #2, lsl #12",
                        "b.hs 7f",
                        "lsl x8, x8, #12",
                        "add x9, x12, x8",
                        "cmp x18, x9",
                        "b.eq 21f",
                        "mov x18, x9",
                        "21:",
                        "ldr x1, [x18, #{w}]",
                        "tst w1, w2",
                        "b.eq 7f",
                        "ldr x0, [x1, #-8]",
                        "sub x1, x1, #8",
                        "str x1, [x18, #{w}]",
                        END_TEXT
                        SLABS = sym crate::front::SLABS,
                        w = const $p * 8,
                        in("x2") mask,
                        out("x0") ret,
                        out("x1") _,
                        out("x8") _,
                        out("x9") _,
                        out("x10") _,
                        out("x11") _,
                        out("x12") _,
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
                        "adrp x12, {SLABS}",
                        ARM_TEXT
                        "mov x4, #0",
                        "ldr w8, [x10, #4]",
                        "cmp w8, #2, lsl #12",
                        "b.hs 7f",
                        "lsl x8, x8, #12",
                        "add x9, x12, x8",
                        "cmp x18, x9",
                        "b.eq 21f",
                        "mov x18, x9",
                        "21:",
                        "ldr x1, [x18, #{w}]",
                        "add x9, x1, #8",
                        "tst w9, w2",
                        "b.eq 7f",
                        "str x0, [x1]",
                        "mov x4, x9",
                        "str x9, [x18, #{w}]",
                        END_TEXT
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
                        out("x12") _,
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
""".replace("POSITIONS", positions).replace("ARM_TEXT", ARM).replace("END_TEXT", END)
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
elif variant == "area":
    s = text[inline]
    cut = s.index("pub(crate) unsafe fn current_cpu")
    head, tail = s[:cut], s[cut:]
    head = sub(head, r"ldr x9, [x11, #32]\nldr x9, [x9]\nmrs x10, tpidr_el0\nadd x10, x10, x9\n", "", 2)
    head = sub(head, r".quad {__rseq_offset}\n", "", 2)
    head = sub(head, "            __rseq_offset = sym crate::front::__rseq_offset,\n", "", 2)
    head = sub(head, 'out("x10") _,', 'in("x10") area(),', 4)
    text[inline] = head + tail + AREA
elif variant == "relaxed":
    text["back/word.rs"] = text["back/word.rs"].replace("SeqCst", "Relaxed").replace("Release", "Relaxed")
    text["back/word.rs"] = text["back/word.rs"].replace("{Relaxed, Relaxed}", "Relaxed")
elif variant == "hugen":
    # One binary whose huge-page switch comes from CRALLOC_HUGE_N, read with the allocator's one
    # environment setting, at the first slow path, before the first carve.
    text["back/policy.rs"] += '''
pub static HUGE_AFTER_N: core::sync::atomic::AtomicUsize =
    core::sync::atomic::AtomicUsize::new(HUGE_AFTER_HUGE_PAGES);

pub fn huge_after_switched(region_huge_pages: usize) -> usize {
    let quarter = region_huge_pages / 4;
    let quarter = if quarter == 0 { 1 } else { quarter };
    quarter.min(HUGE_AFTER_N.load(core::sync::atomic::Ordering::Relaxed))
}
'''
    s = text["back/region.rs"]
    s = sub(s, "let threshold = huge_after(region_huge_pages as usize) as u32 * layout.huge_pages();",
            "let threshold = super::policy::huge_after_switched(region_huge_pages as usize) as u32 * layout.huge_pages();")
    s = s.replace("    grown_huge, huge_after, wants_huge,", "    grown_huge, wants_huge,")
    text["back/region.rs"] = s
    s = text["environment.rs"]
    s = sub(s, """fn read<O: Os>(os: &O) -> Choice {
    let mut piece = [0u8; PIECE];
""", """fn read<O: Os>(os: &O) -> Choice {
    let mut piece = [0u8; PIECE];
    {
        const NAME_N: &[u8] = b\"CRALLOC_HUGE_N=\";
        // At the start of an entry, how far the name has matched; past it, the number so far.
        let (mut matched, mut other, mut value) = (0usize, false, 0usize);
        os.environment(&mut piece, |bytes| {
            for &byte in bytes {
                if byte == 0 {
                    if matched == NAME_N.len() {
                        return false;
                    }
                    (matched, other) = (0, false);
                } else if matched == NAME_N.len() {
                    value = value * 10 + (byte - b'0') as usize;
                } else if !other && NAME_N.get(matched) == Some(&byte) {
                    matched += 1;
                } else {
                    other = true;
                }
            }
            true
        });
        if value != 0 {
            crate::back::policy::HUGE_AFTER_N.store(value, Relaxed);
        }
    }
""")
    text["environment.rs"] = s
elif variant == "huge1":
    text["back/policy.rs"] = sub(text["back/policy.rs"], "pub const HUGE_AFTER_HUGE_PAGES: usize = 8;",
                                 "pub const HUGE_AFTER_HUGE_PAGES: usize = 1;")
elif variant == "nostretch":
    s = text["back/class.rs"]
    s = sub(s, "const STRETCH_MISSES: u32 = 2;", "const STRETCH_MISSES: u32 = 0;")
    a = s.index("        // Two at a time, each from a pointer stepping two strides")
    b = s.index("    #[inline(always)]\n    fn put(&mut self, at: usize, slot: *mut u8) {\n        let index")
    s = s[:a] + """        let mut next = first;
        for slot in part.iter().rev() {
            slot.store(next, Relaxed);
            next = next.wrapping_byte_add(stride);
        }
    }

""" + s[b:]
    text["back/class.rs"] = s
elif variant != "plain":
    sys.exit(f"unknown variant {variant}")

for name, path in files.items():
    open(path, "w").write(text[name])
