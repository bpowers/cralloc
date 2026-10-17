#!/usr/bin/env python3
"""Generate the code-layout sweep kernels.

Run by build.rs; writes one Rust source file (a `global_asm!` block, the extern
declarations and a table of specs) to the path given as the first argument.

Three families:

  pair    the bare magazine pop+push pair (11 fused uops, 51 bytes) with the loop
          start shifted by k bytes, or with nops inserted at chosen positions
  loop    a loop whose whole body is filler plus `dec rdi; jnz`: n fused uops per
          iteration, with no dependency chain or with a 2- or 3-cycle register chain
  callee  a loop that calls a function of n uops ending in `ret`, optionally placed
          at an offset inside its 64-byte line
"""
import sys

# The bare magazine pair: rbx = slab base, r12 = class offset, rax = item.
PAIR = [
    "mov rcx, qword ptr [rbx + r12]",  # 0  load top
    "sub rcx, 8",                      # 1
    "test ecx, 0x1f8",                 # 2
    "jz 9f",                           # 3  (fuses with 2)
    "mov rax, qword ptr [rcx]",        # 4  load slot
    "mov qword ptr [rbx + r12], rcx",  # 5  commit pop
    "mov rcx, qword ptr [rbx + r12]",  # 6  load top
    "test ecx, 0x1f8",                 # 7
    "jz 9f",                           # 8  (fuses with 7)
    "mov qword ptr [rcx], rax",        # 9  store slot
    "add rcx, 8",                      # 10
    "mov qword ptr [rbx + r12], rcx",  # 11 commit push
    "dec rdi",                         # 12
    "jnz 2b",                          # 13 (fuses with 12)
]

# Single-instruction nops of each length, so bytes and uops can be varied separately.
NOP = {
    1: "nop",
    2: ".byte 0x66, 0x90",
    3: ".byte 0x0f, 0x1f, 0x00",
    4: ".byte 0x0f, 0x1f, 0x40, 0x00",
    5: ".byte 0x0f, 0x1f, 0x44, 0x00, 0x00",
    6: ".byte 0x66, 0x0f, 0x1f, 0x44, 0x00, 0x00",
    7: ".byte 0x0f, 0x1f, 0x80, 0x00, 0x00, 0x00, 0x00",
    8: ".byte 0x0f, 0x1f, 0x84, 0x00, 0x00, 0x00, 0x00, 0x00",
    9: ".byte 0x66, 0x0f, 0x1f, 0x84, 0x00, 0x00, 0x00, 0x00, 0x00",
}

LEA = ["lea r10, [r11 + 1]", "lea r8, [r11 + 2]", "lea rcx, [r11 + 3]", "lea rdx, [r11 + 4]"]  # 4 bytes each
MOV = ["mov eax, esi", "mov ecx, esi", "mov edx, esi"]  # 2 bytes each


def fill(pool, n):
    return [pool[i % len(pool)] for i in range(n)]


pairs = []    # (name, shift, {position: [lines inserted before it]}, description)
loops = []    # (name, body lines, description)
callees = []  # (name, callee body lines, entry offset, description)

for k in range(64):
    pairs.append((f"la_shift{k:02d}", k, {}, f"pair, loop start at 64n+{k}"))
for i in range(14):
    pairs.append((f"lb_nop_at{i:02d}", 0, {i: ["nop"]}, f"pair, one nop before instruction {i}"))
for n in range(1, 10):
    pairs.append((f"lc_top_nop{n}b", 0, {0: [NOP[n]]}, f"pair, one {n}-byte nop at top"))
for m in range(13):
    pairs.append((f"ld_top_{m:02d}nops", 0, {0: ["nop"] * m}, f"pair, {m} one-byte nops at top"))
for m in range(9):
    pairs.append((f"le_mid_{m:02d}nops", 0, {6: ["nop"] * m}, f"pair, {m} nops between pop commit and push load"))
for m in range(9):
    pairs.append((f"lf_end_{m:02d}nops", 0, {12: ["nop"] * m}, f"pair, {m} nops between push commit and dec/jnz"))
for m in range(7):
    pairs.append((f"lg_both_{m:02d}nops", 0, {6: ["nop"] * m, 12: ["nop"] * m}, f"pair, {m} nops after each commit"))

for n in range(2, 31):
    loops.append((f"ln_{n:02d}uops", ["nop"] * (n - 1), f"loop of {n} uops: nops, no chain"))
for n in range(2, 31):
    loops.append((f"lm_{n:02d}uops", fill(MOV, n - 1), f"loop of {n} uops: 2-byte movs, no chain"))
for n in range(3, 27):
    loops.append((f"lr2_{n:02d}uops", ["add rax, 1"] * 2 + ["nop"] * (n - 3), f"loop of {n} uops: 2-cycle chain + nops"))
for n in range(4, 27):
    loops.append((f"lr3_{n:02d}uops", ["add rax, 1"] * 3 + ["nop"] * (n - 4), f"loop of {n} uops: 3-cycle chain + nops"))
for n in range(3, 27):
    loops.append((f"lw2_{n:02d}uops", ["add rax, 1"] * 2 + fill(LEA, n - 3), f"loop of {n} uops: 2-cycle chain + 4-byte leas"))

for n in range(1, 31):
    callees.append((f"lcn_{n:02d}", ["nop"] * (n - 1), 0, f"call a {n}-uop function: nops"))
for n in range(1, 31):
    callees.append((f"lcm_{n:02d}", fill(MOV, n - 1), 0, f"call a {n}-uop function: 2-byte movs ({2 * (n - 1) + 1} bytes)"))
for n in range(1, 31):
    callees.append((f"lcl_{n:02d}", fill(LEA, n - 1), 0, f"call a {n}-uop function: 4-byte leas ({4 * (n - 1) + 1} bytes)"))
for off in range(0, 64, 4):
    callees.append((f"lcq_{off:02d}", fill(LEA, 5), off, f"call a 6-uop 21-byte function at 64n+{off}"))
for off in range(0, 64, 4):
    callees.append((f"lco_{off:02d}", fill(LEA, 10), off, f"call an 11-uop 41-byte function at 64n+{off}"))
for off in range(0, 64, 4):
    callees.append((f"lcp_{off:02d}", fill(LEA, 15), off, f"call a 16-uop 61-byte function at 64n+{off}"))

asm = []
specs = []


def function(name, prologue, body, epilogue):
    head = [".text", ".balign 64", f".globl {name}", f".type {name},@function", f"{name}:"]
    asm.append("\n".join(head + prologue + body + epilogue + [f".size {name}, .-{name}"]))


for name, shift, inserted, desc in pairs:
    body = [".balign 64"] + ([f".skip {shift}, 0x90"] if shift else []) + ["2:"]
    for i, line in enumerate(PAIR):
        body += inserted.get(i, []) + [line]
    function(name, ["push rbx", "push r12", "mov rbx, rsi", "mov rsi, rcx", "mov r12, r8"], body,
             ["pop r12", "pop rbx", "ret", "9:", "ud2"])
    specs.append((name, desc))

for name, body, desc in loops:
    function(name, ["xor r11d, r11d"], [".balign 64", "2:"] + body + ["dec rdi", "jnz 2b"], ["ret"])
    specs.append((name, desc))

for name, body, off, desc in callees:
    pad = [f".skip {off}, 0xcc"] if off else []
    asm.append("\n".join([".text", ".balign 64"] + pad + [f"{name}_callee:"] + body + ["ret"]))
    function(name, ["xor r11d, r11d"], [".balign 64", "2:", f"call {name}_callee", "dec rdi", "jnz 2b"], ["ret"])
    specs.append((name, desc))

with open(sys.argv[1], "w") as f:
    f.write('std::arch::global_asm!(r#"\n' + "\n\n".join(asm) + '\n"#);\n')
    f.write('extern "C" {\n')
    for name, _ in specs:
        f.write(f"    fn {name}(iters: u64, state: *mut u8, aux: u64, arr: *mut u8, class: u64) -> u64;\n")
    f.write("}\n")
    f.write("pub fn specs() -> Vec<(&'static str, crate::kernels::Kernel, &'static str)> {\n    vec![\n")
    for name, desc in specs:
        f.write(f'        ("{name}", {name} as crate::kernels::Kernel, "{desc}"),\n')
    f.write("    ]\n}\n")
