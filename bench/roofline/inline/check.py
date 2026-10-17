#!/usr/bin/env python3
"""Build the inlining study in every profile and report, per allocation site, whether the
allocator fast path was inlined.

Usage: ./check.py [variant ...]      variants: rust rust+used asm asm+used asm+modasm asm+used+sep (default: all)

For each (variant, profile) it builds `app`, disassembles it and looks at four functions:

  bin      app::site_bin            a site in the crate that declares #[global_allocator]
  bin2     app::other::site_other   the same in another module (another codegen unit)
  dep      dep::pair                a non-generic site compiled in a dependency crate
  generic  dep::pair_generic<u64>   a generic function instantiated in the bin crate
  std      the allocation that `format!` makes inside the precompiled standard library

A site counts as inlined when the function has no call (direct, or indirect through the GOT)
to __rust_alloc or __rust_dealloc. The shim column says whether a call to
__rust_no_alloc_shim_is_unstable_v2 is still made. The last columns are cycles per alloc+free
pair measured by the binary itself.
"""
import os
import re
import subprocess
import sys

PROFILES = ["off", "release", "cgu1", "thin", "thin1", "fat"]
VARIANTS = {
    "rust": [],
    "rust+used": ["used"],
    "asm": ["asm"],
    "asm+used": ["asm", "used"],
    "asm+modasm": ["asm", "modasm"],
    "asm+used+sep": ["asm", "used", "sep"],
}
SITES = {
    "bin": re.compile(r"^app::site_bin$"),
    "bin2": re.compile(r"^app::other::site_other$"),
    "dep": re.compile(r"^dep::pair$"),
    "generic": re.compile(r"^dep::pair_generic::<u64>$"),
}
ALLOC = ("__rust_alloc", "__rust_dealloc", "__rust_realloc", "__rust_alloc_zeroed")
SHIM = "__rust_no_alloc_shim_is_unstable"


def run(cmd, **kw):
    return subprocess.run(cmd, check=True, capture_output=True, text=True, **kw).stdout


def got_targets(binary):
    """GOT slot address -> address the slot is relocated to (R_X86_64_RELATIVE only)."""
    slots = {}
    for line in run(["readelf", "-rW", binary]).splitlines():
        f = line.split()
        if len(f) >= 4 and f[2] == "R_X86_64_RELATIVE":
            slots[int(f[0], 16)] = int(f[3], 16)
    return slots


def functions(binary):
    """name -> (address, [instruction text]) from a demangled disassembly."""
    out = {}
    name = None
    for line in run(["objdump", "-d", "-C", "--no-show-raw-insn", "-M", "intel", binary]).splitlines():
        m = re.match(r"^([0-9a-f]+) <(.*)>:$", line)
        if m:
            name = m.group(2)
            out[name] = (int(m.group(1), 16), [])
        elif name and "\t" in line:
            out[name][1].append(line.split("\t", 1)[1].strip())
    return out


def calls(insns, slots, by_addr):
    """Names of everything a function calls, resolving loads of GOT slots it later calls through."""
    found = set()
    for i in insns:
        m = re.match(r"(?:call|jmp)\s+[0-9a-f]+ <(.*?)(?:\+0x[0-9a-f]+)?>$", i)
        if m and i.startswith("call"):
            found.add(m.group(1))
        m = re.search(r"\[rip\+0x[0-9a-f]+\]\s+# ([0-9a-f]+) <", i)
        if m and (i.startswith("call") or i.startswith("mov")):
            target = slots.get(int(m.group(1), 16))
            if target in by_addr:
                found.add(by_addr[target])
    return found


def short(names, needles):
    return any(n.split("::")[-1].startswith(needles) or any(x in n for x in needles) for n in names)


def analyse(binary):
    slots = got_targets(binary)
    funcs = functions(binary)
    by_addr = {addr: name for name, (addr, _) in funcs.items()}
    row = {}
    for site, pat in SITES.items():
        hit = [n for n in funcs if pat.match(n)]
        if not hit:
            row[site] = "missing"
            continue
        c = calls(funcs[hit[0]][1], slots, by_addr)
        inlined = not short(c, ALLOC)
        row[site] = ("inlined" if inlined else "call") + (" +shim" if short(c, (SHIM,)) else "")
    # std: every function from the precompiled standard library that still calls the allocator
    # out of line. `format!` reaches the allocator through such functions.
    std_callers = [n for n, (_, insns) in funcs.items()
                   if re.match(r"^<?(alloc|std|core)::", n) and short(calls(insns, slots, by_addr), ALLOC)]
    fmt = [n for n in funcs if "fmt::format::format_inner" in n]
    if fmt:
        c = calls(funcs[fmt[0]][1], slots, by_addr)
        row["std"] = ("inlined" if not short(c, ALLOC) else "call") + (" +shim" if short(c, (SHIM,)) else "")
    else:
        row["std"] = "no format_inner"
    row["std callers"] = str(len(std_callers))
    return row


def main():
    here = os.path.dirname(os.path.abspath(__file__))
    wanted = sys.argv[1:] or list(VARIANTS)
    cols = ["bin", "bin2", "dep", "generic", "std", "std callers"]
    print("| fast path | profile | " + " | ".join(cols) + " | cyc bin | cyc bin2 | cyc dep | cyc generic | cyc std |")
    print("|---|---|" + "---|" * len(cols) + "---:|---:|---:|---:|---:|")
    for variant in wanted:
        for profile in PROFILES:
            target = os.path.join(here, "target-" + variant.replace("+", "-"))
            cmd = ["cargo", "build", "-q", "-p", "app", "--profile", profile, "--target-dir", target]
            if VARIANTS[variant]:
                cmd += ["--features", ",".join(VARIANTS[variant])]
            subprocess.run(cmd, check=True, cwd=here, capture_output=True)
            binary = os.path.join(target, profile, "app")
            row = analyse(binary)
            timing = dict(line.split() for line in run([binary]).splitlines())
            print(f"| {variant} | {profile} | " + " | ".join(row[c] for c in cols)
                  + f" | {timing['bin']} | {timing['bin2']} | {timing['dep']} | {timing['generic']} | {timing['std']} |", flush=True)


if __name__ == "__main__":
    main()
