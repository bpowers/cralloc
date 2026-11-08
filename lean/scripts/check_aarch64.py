#!/usr/bin/env python3
"""Checks that the AArch64 printer prints what the model means. Exits non-zero
on a difference, naming it.

The printer and the model are trusted, and a mistake in either would prove the
wrong thing. So:

  * Each golden instruction (`CrallocTest/PrintAArch64.lean`, `golden`) is
    assembled with `llvm-mc`, the assembler rustc uses, and must take the
    encoding the Arm ARM gives it there: an assembler free to choose has chosen
    what the model describes.
  * `logicalImmOk`, which refuses a logical immediate no instruction encodes,
    must agree with the assembler on thousands of values, in both directions.
  * On an AArch64 host, every instruction form the model has is assembled and run
    on the processor, on the edge values of its operands, from the registers, flags
    and memory each vector gives (`CrallocTest/VectorsAArch64.lean`), and must end
    with the registers, flags and memory the model says. On another host this
    part is skipped, and says so.

Run it from anywhere after `lake build`, as `scripts/check.sh` does. It needs
`llvm-mc` and, for the last part, a C compiler that links a fixed address.
"""

import pathlib
import platform
import re
import shutil
import struct
import subprocess
import sys
import tempfile

LEAN = pathlib.Path(__file__).resolve().parent.parent
TRIPLE = "aarch64-unknown-linux-gnu"
# The modelled registers, in the order the vectors give them.
REGS = [*range(18), *range(19, 29), 30]
FAILS_SHOWN = 20


def vectors() -> list[str]:
    r = subprocess.run(["lake", "env", "lean", "--run", "CrallocTest/VectorsAArch64.lean"],
                       cwd=LEAN, capture_output=True, text=True, check=False)
    if r.returncode != 0:
        raise SystemExit(f"check_aarch64: the vectors did not run:\n{r.stdout}{r.stderr}")
    return r.stdout.splitlines()


def llvm_mc(source: str, *args: str) -> subprocess.CompletedProcess:
    return subprocess.run(["llvm-mc", f"-triple={TRIPLE}", *args], input=source,
                          capture_output=True, text=True, check=False)


def check_golden(golden: list[tuple[int, str]]) -> list[str]:
    """Each line's encoding, as `llvm-mc -show-encoding` prints it, is the one
    the golden table gives."""
    bad = []
    r = llvm_mc("\n".join(t for _, t in golden) + "\n", "-show-encoding")
    if r.returncode != 0:
        return [f"llvm-mc refuses a golden instruction:\n{r.stderr}"]
    found = re.findall(r"encoding: \[0x(..),0x(..),0x(..),0x(..)\]", r.stdout)
    if len(found) != len(golden):
        return [f"llvm-mc encodes {len(found)} of {len(golden)} golden instructions"]
    for (want, text), bs in zip(golden, found):
        got = int("".join(reversed(bs)), 16)
        if got != want:
            bad.append(f"`{text}` encodes as {got:#010x}, the golden table says {want:#010x}")
    return bad


def check_logical(cases: list[tuple[int, int, bool]]) -> list[str]:
    """The assembler accepts `and` of a value exactly where `logicalImmOk` does."""
    source = "".join(f"and {'x' if bits == 64 else 'w'}0, {'x' if bits == 64 else 'w'}1, #{v:#x}\n"
                     for bits, v, _ in cases)
    r = llvm_mc(source, "-filetype=null")
    refused = {int(m) - 1 for m in re.findall(r"^<stdin>:(\d+):", r.stderr, re.M)}
    bad = []
    for i, (bits, v, ok) in enumerate(cases):
        if ok == (i in refused):
            bad.append(f"logicalImmOk says {ok} of {v:#x} on {bits} bits, and the assembler "
                       f"{'refuses' if i in refused else 'accepts'} it")
    return bad


def stub(test: int, kind: str, lines: str) -> str:
    """A function of a context (the registers in, the flags, the thread pointer;
    then the registers and flags out) that loads the context, runs the test and
    saves what it ends with. x18 and x29 are not modelled: x29 finds the context
    again, which no test can disturb."""
    body = lines.split(";")
    if kind == "B":
        body += ["mov x17, #0", "b 96f", "97:", "mov x17, #1", "96:"]
    load = [f"ldr x{r}, [x0, #{8 * i}]" for i, r in enumerate(REGS) if r != 0]
    save = [f"str x{r}, [x29, #{8 * (31 + i)}]" for i, r in enumerate(REGS)]
    return "\n".join([
        ".text", ".p2align 2", f".globl vt_{test}", f"vt_{test}:",
        "stp x29, x30, [sp, #-112]!", "stp x19, x20, [sp, #16]", "stp x21, x22, [sp, #32]",
        "stp x23, x24, [sp, #48]", "stp x25, x26, [sp, #64]", "stp x27, x28, [sp, #80]",
        "str x0, [sp, #96]", "mrs x1, tpidr_el0", "str x1, [sp, #104]",
        f"ldr x1, [x0, #{8 * 30}]", "msr tpidr_el0, x1",
        f"ldr x1, [x0, #{8 * 29}]", "msr nzcv, x1",
        *load, "ldr x0, [x0]",
        *body,
        "ldr x29, [sp, #96]", *save,
        "mrs x0, nzcv", f"str x0, [x29, #{8 * 60}]",
        "ldr x0, [sp, #104]", "msr tpidr_el0, x0",
        "ldp x19, x20, [sp, #16]", "ldp x21, x22, [sp, #32]", "ldp x23, x24, [sp, #48]",
        "ldp x25, x26, [sp, #64]", "ldp x27, x28, [sp, #80]", "ldp x29, x30, [sp], #112",
        "ret",
        '.pushsection .vgsym,"aw",%progbits', "3:", ".popsection", ""])


HARNESS = r"""
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#define BUF 0x7000000000ULL
#define LEN 0x10000
typedef struct { uint32_t test, nchanged; uint64_t nzcv_in, in[29], nzcv_out, out[29]; } vec;
typedef struct { uint64_t addr; uint64_t byte; } chg;
%(decls)s
static void (*const tests[])(uint64_t *) = { %(table)s };
static uint8_t expect[LEN];
int main(int argc, char **argv) {
  FILE *f = fopen(argv[1], "rb");
  if (!f) { perror("vectors"); return 2; }
  uint8_t *buf = mmap((void *)BUF, LEN, PROT_READ | PROT_WRITE,
                      MAP_PRIVATE | MAP_ANONYMOUS | MAP_FIXED_NOREPLACE, -1, 0);
  if (buf != (void *)BUF) { perror("mmap"); return 2; }
  for (uint64_t i = 0; i < LEN; i++) {
    uint64_t a = BUF + i;
    expect[i] = buf[i] = (a * 0x9d + (a >> 8) * 0x3b) %% 251;
  }
  long count = 0, bad = 0;
  vec v;
  while (fread(&v, sizeof v, 1, f) == 1) {
    uint64_t ctx[61];
    memcpy(ctx, v.in, sizeof v.in);
    ctx[29] = v.nzcv_in << 28;
    ctx[30] = %(tp)s;
    chg c[64];
    if (v.nchanged > 64 || fread(c, sizeof c[0], v.nchanged, f) != v.nchanged) return 2;
    for (uint32_t i = 0; i < v.nchanged; i++) expect[c[i].addr - BUF] = c[i].byte;
    tests[v.test](ctx);
    int fail = ctx[60] >> 28 != v.nzcv_out || (ctx[60] & 0xfffffff);
    for (int i = 0; i < 29; i++) fail |= ctx[31 + i] != v.out[i];
    fail |= memcmp(buf, expect, LEN) != 0;
    if (fail && bad++ < %(shown)d) {
      printf("test %%u: flags %%llx in; model %%llx, cpu %%llx\n", v.test,
             (unsigned long long)v.nzcv_in, (unsigned long long)v.nzcv_out,
             (unsigned long long)ctx[60] >> 28);
      for (int i = 0; i < 29; i++)
        if (ctx[31 + i] != v.out[i])
          printf("  register %%d: in %%llx; model %%llx, cpu %%llx\n", i,
                 (unsigned long long)v.in[i], (unsigned long long)v.out[i],
                 (unsigned long long)ctx[31 + i]);
      for (uint64_t i = 0; i < LEN; i++)
        if (buf[i] != expect[i])
          printf("  byte %%llx: model %%x, cpu %%x\n", (unsigned long long)(BUF + i), expect[i], buf[i]);
    }
    for (uint64_t i = 0; i < LEN; i++) {
      uint64_t a = BUF + i;
      if (buf[i] != expect[i] || 1) expect[i] = buf[i] = (a * 0x9d + (a >> 8) * 0x3b) %% 251;
    }
    count++;
  }
  printf("%%ld vectors, %%ld differ\n", count, bad);
  return bad != 0;
}
"""


def check_hardware(lines: list[str]) -> list[str]:
    """Runs every test on this processor."""
    tests: list[tuple[int, str, str]] = []
    data = bytearray()
    n = 0
    bad = []
    for line in lines:
        if line.startswith("T "):
            _, tid, kind, text = line.split(" ", 3)
            tests.append((int(tid), kind, text))
        elif line.startswith("F "):
            bad.append(f"the model faults on test {line.split()[1]}: {line}")
        elif line.startswith("V "):
            head, ins, outs = line.split(" | ") if line.count(" | ") == 2 else (line, "", "")
            parts = head.split()
            tid, nzcv_in = int(parts[1]), int(parts[2], 16)
            regs_in = [int(x, 16) for x in parts[3:]]
            out = ins.split()
            nzcv_out, regs_out = int(out[0], 16), [int(x, 16) for x in out[1:]]
            changed = [tuple(int(x, 16) for x in c.split(":")) for c in outs.split(",") if c]
            data += struct.pack("<IIQ29QQ29Q", tid, len(changed), nzcv_in, *regs_in, nzcv_out, *regs_out)
            for addr, byte in changed:
                data += struct.pack("<QQ", addr, byte)
            n += 1
    if bad:
        return bad
    with tempfile.TemporaryDirectory() as d:
        tmp = pathlib.Path(d)
        asm = ['.section .vgsym,"aw",%progbits', ".skip 0x1238", ".globl vg_sym", "vg_sym:",
               ".skip 0x2460 - 0x1238",
               # The symbol `addAreaHi` and `addAreaLo` name, as `AREA_TPREL` says it lies.
               '.section .tdata,"awT",%progbits', ".balign 4096", ".skip 768",
               ".globl cralloc_rseq_area", "cralloc_rseq_area:", ".long 0"]
        for tid, kind, text in tests:
            asm.append(stub(tid, kind, text))
        asm += ['.section .vgsym,"aw",%progbits', ".skip 32", ".quad vg_sym",
                '.section .note.GNU-stack,"",%progbits']
        (tmp / "t.s").write_text("\n".join(asm) + "\n")
        (tmp / "t.c").write_text(HARNESS % {
            "decls": "\n".join(f"void vt_{t}(uint64_t *);" for t, _, _ in tests),
            "table": ", ".join(f"vt_{t}" for t, _, _ in tests),
            "tp": "0x7654321000ULL",
            "shown": FAILS_SHOWN})
        (tmp / "v.bin").write_bytes(data)
        cc = shutil.which("cc") or shutil.which("gcc") or shutil.which("clang")
        r = subprocess.run([cc, "-O1", "-no-pie", str(tmp / "t.c"), str(tmp / "t.s"), "-o", str(tmp / "t"),
                            "-Wl,--section-start=.vgsym=0x20000000"], capture_output=True, text=True, check=False)
        if r.returncode != 0:
            return [f"the harness does not build:\n{r.stdout}{r.stderr}"]
        r = subprocess.run([str(tmp / "t"), str(tmp / "v.bin")], capture_output=True, text=True, check=False)
        print(f"check_aarch64: {len(tests)} instruction forms on this processor: {r.stdout.strip().splitlines()[-1] if r.stdout.strip() else ''}")
        if r.returncode != 0:
            names = {t: f"{k} {x}" for t, k, x in tests}
            shown = re.sub(r"test (\d+):", lambda m: f"`{names[int(m[1])]}`:", r.stdout)
            return [f"the model and this processor differ:\n{shown}{r.stderr}"]
    return []


def main() -> int:
    if shutil.which("llvm-mc") is None:
        print("check_aarch64: llvm-mc is needed", file=sys.stderr)
        return 1
    lines = vectors()
    golden = [(int(l.split(" ", 2)[1], 16), l.split(" ", 2)[2]) for l in lines if l.startswith("G ")]
    logical = [(int(p[1]), int(p[2], 16), p[3] == "1") for p in (l.split() for l in lines if l.startswith("L "))]
    bad = check_golden(golden) + check_logical(logical)
    print(f"check_aarch64: {len(golden)} golden encodings, {len(logical)} logical immediates")
    if platform.machine() == "aarch64":
        bad += check_hardware(lines)
    else:
        print("check_aarch64: not on an AArch64 host, so the model was not run against a processor")
    for b in bad:
        print(b, file=sys.stderr)
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
