#!/usr/bin/env python3
"""Tables of where the memory is, from the accounted runs of runs.sh (docs/research/trough-gap.md).

    python3 -I decomp.py RUN.txt [--classes TAG] [--regions TAG]

RUN.txt is a drift run of `tg-stat-acct` (with RUN.txt.memstat beside it) or `je-stat-acct`
(RUN.txt.je). Account tag k is the end of phase k; the last tag is the end of the quiet minute.
Prints, in MiB, for each tag: the program's live bytes, the heap (RssAnon less the workload's
books), and the allocator's parts, which add up to the heap but for what the account does not
see (the residual: page tables are not in RssAnon; the residual is the sampler's own buffers
for asmalloc and memory jemalloc does not report for jemalloc). With --classes or --regions,
asmalloc's parts by class or region at that tag.
"""
import json
import re
import sys
from pathlib import Path

MIB = 1 << 20


def kv(line):
    return {k: (int(v) if re.fullmatch(r"-?\d+", v) else v) for k, v in re.findall(r"(\S+?)=(\S*)", line)}


def drift_accounts(path):
    """The DRIFT samples that took an account, in tag order."""
    out = []
    for line in Path(path).read_text().splitlines():
        if line.startswith("DRIFT "):
            d = json.loads(line[6:])
            if d.get("account", -1) > 0:
                out.append(d)
    return out


def asm(path, samples, args):
    lines = Path(str(path) + ".memstat").read_text().splitlines()
    tags, cur = {}, None
    for line in lines:
        if line.startswith("account tag="):
            cur = int(line.split("=")[1])
            tags[cur] = {"classes": [], "regions": []}
        elif cur is None:
            continue
        elif line.startswith("acct-decomp "):
            tags[cur]["d"] = kv(line)
        elif line.startswith("acct-dclass "):
            tags[cur]["classes"].append(kv(line))
        elif line.startswith("acct-dregion "):
            tags[cur]["regions"].append(kv(line))
    cols = [
        ("phase", None), ("live", None), ("heap", None), ("slots", "live"), ("frag", "free_on_live"),
        ("parked", None), ("free_by_parked", "free_on_parked"), ("sp_below", "sp_below"),
        ("sp_wait", None), ("hp_below", None), ("hp_quarter", "hp_quarter"), ("hp_wait", None),
        ("hp_split", None), ("above", "above"), ("meta", None), ("resid", None), ("live_pg", None),
        ("thp", None),
    ]
    print(" ".join(f"{c:>8}" for c, _ in cols))
    for tag in sorted(tags):
        t = tags[tag]
        if "d" not in t:
            continue
        d = t["d"]
        s = samples[tag - 1] if tag - 1 < len(samples) else None
        parked = d["park_on_live"] + d["park_alone"]
        meta = d["meta"] + d["mags"] + d["slabs"]
        parts = {
            "phase": (s["phase"] if s else "?")[:8] if s and s["kind"] == "load" else "quiet",
            "live": s["live"] / MIB if s else float("nan"),
            "heap": s["heap"] / MIB if s else float("nan"),
            "slots": d["live"] / MIB,
            "frag": d["free_on_live"] / MIB,
            "parked": parked / MIB,
            "free_by_parked": d["free_on_parked"] / MIB,
            "sp_below": d["sp_below"] / MIB,
            "sp_wait": (d["sp_long"] + d["sp_short"]) / MIB,
            "hp_below": (d["hp_below"] + d["hp_free_below"]) / MIB,
            "hp_quarter": d["hp_quarter"] / MIB,
            "hp_wait": (d["hp_sparse_long"] + d["hp_sparse_short"] + d["hp_free_above"]) / MIB,
            "hp_split": (d["hp_split_long"] + d["hp_split_short"]) / MIB,
            "above": d["above"] / MIB,
            "meta": meta / MIB,
            "live_pg": d["live_pages"] * 4096 / MIB,
            "thp": d["thp"] / MIB,
        }
        acct = d["resident"] + meta + d["runs"]
        parts["resid"] = (s["heap"] - acct) / MIB if s else float("nan")
        print(" ".join(f"{parts[c]:>8.1f}" if not isinstance(parts[c], str) else f"{parts[c]:>8}" for c, _ in cols))
    if "--classes" in args:
        tag = int(args[args.index("--classes") + 1])
        print(f"-- classes at tag {tag}: size slots frag parked sp_below sp_wait hp_below hp_quarter hp_wait hp_split live_pg")
        for c in tags[tag]["classes"]:
            print(f"{c['size']:>6} " + " ".join(f"{x / MIB:8.1f}" for x in [
                c["live"], c["free_on_live"], c["park_on_live"] + c["park_alone"], c["sp_below"],
                c["sp_long"] + c["sp_short"], c["hp_below"] + c["hp_free_below"], c["hp_quarter"],
                c["hp_sparse_long"] + c["hp_sparse_short"] + c["hp_free_above"],
                c["hp_split_long"] + c["hp_split_short"], c["live_pages"] * 4096]))
    if "--regions" in args:
        tag = int(args[args.index("--regions") + 1])
        for r in tags[tag]["regions"]:
            print(f"size={r['size']} carved={r['carved_mib']} huge_from={r['huge_from_mib']} frontier={r['frontier_mib']} "
                  f"tail={r['tail_from_mib']} live={r['live'] / MIB:.1f} frag={r['free_on_live'] / MIB:.1f} "
                  f"hp_below={(r['hp_below'] + r['hp_free_below']) / MIB:.1f} q={r['hp_quarter'] / MIB:.1f} "
                  f"wait={(r['hp_sparse_long'] + r['hp_sparse_short'] + r['hp_free_above']) / MIB:.1f} "
                  f"split={(r['hp_split_long'] + r['hp_split_short']) / MIB:.1f} map={r.get('map', '')}")


def je(path, samples, args):
    text = Path(str(path) + ".je").read_text().splitlines()
    accts, bins = {}, {}
    for line in text:
        if line.startswith("je-acct "):
            d = kv(line)
            accts[d["tag"]] = d
        elif line.startswith("je-bin "):
            d = kv(line)
            bins.setdefault(d["tag"], []).append(d)
    cols = ["phase", "live", "heap", "alloc", "tcache", "slabfree", "slabpg", "dirty", "muzzy", "meta", "resid"]
    print(" ".join(f"{c:>8}" for c in cols))
    for tag in sorted(accts):
        a = accts[tag]
        s = samples[tag - 1] if tag - 1 < len(samples) else None
        bs = bins.get(tag, [])
        slab_pages = sum(b["curslabs"] * b["slab_size"] for b in bs)
        slab_regs = sum(b["curregs"] * b["size"] for b in bs)
        dirty = a["a.pdirty"] * 4096
        muzzy = a["a.pmuzzy"] * 4096
        parts = {
            "phase": (s["phase"][:8] if s and s["kind"] == "load" else "quiet"),
            "live": s["live"] / MIB if s else float("nan"),
            "heap": s["heap"] / MIB if s else float("nan"),
            "alloc": a["allocated"] / MIB,
            "tcache": a["a.tcache_bytes"] / MIB,
            "slabfree": (slab_pages - slab_regs) / MIB,
            "slabpg": slab_pages / MIB,
            "dirty": dirty / MIB,
            "muzzy": muzzy / MIB,
            "meta": a["metadata"] / MIB,
        }
        parts["resid"] = (s["heap"] - a["active"] - dirty - muzzy - a["metadata"]) / MIB if s else float("nan")
        print(" ".join(f"{parts[c]:>8.1f}" if not isinstance(parts[c], str) else f"{parts[c]:>8}" for c in cols))


def main():
    path, args = sys.argv[1], sys.argv[2:]
    samples = drift_accounts(path)
    if Path(path + ".memstat").exists():
        asm(path, samples, args)
    elif Path(path + ".je").exists():
        je(path, samples, args)
    else:
        sys.exit("no account beside the run")


if __name__ == "__main__":
    main()
