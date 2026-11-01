#!/usr/bin/env python3
"""Fails if cralloc's fast path stops inlining into its callers under fat LTO.

Builds src/main.rs (fat LTO, one codegen unit) with and without cralloc's `executable`
feature, disassembles each `site_*` function and checks its hot loop, from the target of its
first backward branch to that branch: it must hold both restartable sequences (two arming
stores to `rseq_cs` and two CPU-number compares) and no call. The calls to the slow paths sit
in blocks LLVM lays out after the hot loop's branch; the abort handlers and miss stubs are in
`.text.unlikely` and the descriptors in `__rseq_cs`. Prints the hot loop's bytes and
instructions.

    taskset -c 4-7 ./check.py          # about ten seconds once dependencies are built

Needs objdump and nm (binutils).
"""
import pathlib
import re
import subprocess
import sys

HERE = pathlib.Path(__file__).resolve().parent
SITES = ["site_constant", "site_box", "site_runtime"]
ARM = re.compile(r"mov\s+QWORD PTR fs:\[r10\+0x8\],r11")
CPU = re.compile(r"cmp\s+r8d,0x2000")
INSN = re.compile(r"^\s*([0-9a-f]+):\s+(.*)$")
BRANCH = re.compile(r"^j(?!mp)\w*\s+([0-9a-f]+)\b")


def build(features: list[str]) -> pathlib.Path:
    target = HERE / "target" / ("exe" if features else "got")
    cmd = ["cargo", "build", "-q", "--release", "--target-dir", str(target)]
    if features:
        cmd += ["--features", ",".join(features)]
    subprocess.run(cmd, cwd=HERE, check=True)
    return target / "release" / "roofline-fastpath"


def function(binary: pathlib.Path, name: str) -> list[tuple[int, str]]:
    syms = subprocess.run(["nm", "-S", "-C", str(binary)], check=True, capture_output=True,
                          text=True).stdout
    for line in syms.splitlines():
        parts = line.split(maxsplit=3)
        if len(parts) == 4 and parts[3].endswith(f"::{name}"):
            start, size = int(parts[0], 16), int(parts[1], 16)
            break
    else:
        raise SystemExit(f"{binary}: no function {name} (inlined into main?)")
    dis = subprocess.run(["objdump", "-d", "-M", "intel", "--no-show-raw-insn", "-C",
                          f"--start-address={start}", f"--stop-address={start + size}",
                          str(binary)], check=True, capture_output=True, text=True).stdout
    out = []
    for line in dis.splitlines():
        m = INSN.match(line)
        if m:
            out.append((int(m.group(1), 16), m.group(2).strip()))
    return out


def loop(insns: list[tuple[int, str]]) -> list[tuple[int, str]]:
    """The instructions from the target of the first backward conditional branch within the
    function to it (the miss paths branch to `.text.unlikely`, outside it)."""
    start = insns[0][0]
    for i in range(len(insns)):
        addr, text = insns[i]
        m = BRANCH.match(text)
        if m and start <= int(m.group(1), 16) < addr:
            top = int(m.group(1), 16)
            return [x for x in insns[: i + 1] if x[0] >= top]
    return []


def main() -> int:
    bad = []
    for features in ([], ["executable"]):
        binary = build(features)
        form = "direct __rseq_offset" if features else "__rseq_offset by the GOT"
        print(f"{form}:")
        for name in SITES:
            body = loop(function(binary, name))
            if not body:
                bad.append(f"{form}: {name}: no loop found to check")
                continue
            texts = [t for _, t in body]
            arms = sum(bool(ARM.search(t)) for t in texts)
            cpus = sum(bool(CPU.search(t)) for t in texts)
            calls = [t for t in texts if t.startswith("call")]
            nbytes = body[-1][0] - body[0][0]
            print(f"  {name:<14} loop {nbytes:>4} bytes before its branch, {len(body):>3} instructions,"
                  f" {arms} sequences, {len(calls)} calls")
            if arms != 2 or cpus != 2:
                bad.append(f"{form}: {name}: the loop has {arms} arming stores and {cpus} CPU "
                           "compares, not two of each: the fast path is not inlined")
            if calls:
                bad.append(f"{form}: {name}: the loop calls: {calls}")
    for b in bad:
        print(b, file=sys.stderr)
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
