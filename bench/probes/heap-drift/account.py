#!/usr/bin/env python3
"""Where asmalloc's resident heap is, from the breakdowns its memstat build writes
(bench/alloc/src/memstat.rs) at the moments a drift run asks for (`account_every`), for
docs/research/heap-drift.md.

    python3 -I bench/probes/heap-drift/account.py FILE.memstat [--tags 3,7] [--classes N] [--label final]

For each breakdown (all, or those tagged): the resident heap in MiB by category, with free
memory split by what a purge could return now (whole free 4 KiB pages outside a huge-page part,
whole free 2 MiB units inside one) and what it cannot (free slots on pages, or in huge units,
that also hold something); then the N classes holding the most resident memory beyond what the
program asked for in them, with the same split. `req` is what the program asked for, by the
class of its layout. With --label final (or peak), the dump the sampler wrote at exit instead.
"""
import re
import sys
from pathlib import Path

MiB = 2**20
KV = re.compile(r"(\w+)=(\S+)")


def num(v):
    if "/" in v:
        return int(v.split("/")[0])
    try:
        return int(v)
    except ValueError:
        return int(v, 16)


def blocks(path, label):
    """Yields (tag, header, classes) for every dump with this label."""
    next_tag, tag, header, classes = None, None, None, []
    for line in Path(path).read_text().splitlines():
        if line.startswith("account tag="):
            next_tag = int(line.split("=")[1])
            continue
        word = line.split(" ", 1)[0]
        if word == label:
            if header is not None:
                yield tag, header, classes
            tag, header, classes = next_tag, {k: num(v) for k, v in KV.findall(line)}, []
        elif word == f"{label}-class":
            c = {k: v for k, v in KV.findall(line)}
            classes.append({k: (num(v) if "/" not in v else tuple(int(x) for x in v.split("/"))) for k, v in c.items()})
    if header is not None:
        yield tag, header, classes


def split(c):
    """(returnable, stuck) resident free bytes of one class."""
    returnable = c["free_whole"] - c["free_whole_huge"] + c["free_units"]
    free_res = c["free"][0]
    return returnable, free_res - returnable


def report(tag, h, classes, top):
    ret = sum(split(c)[0] for c in classes)
    stuck = sum(split(c)[1] for c in classes)
    req = h["req"]
    small = [c for c in classes if c["size"] <= 32 << 10]
    large = [c for c in classes if c["size"] > 32 << 10]
    huge = sum(c["huge_part"] for c in classes)
    print(f"\n## breakdown {tag} (t={h['t_ms'] / 1000:.1f} s)\n")
    print(f"rss {h['rss'] / MiB:.0f} MiB, heap {h['heap'] / MiB:.0f}, asked for {req / MiB:.0f}: heap/req {h['heap'] / max(req, 1):.3f}")
    rows = [
        ("live slots (incl. class rounding)", h["live"]),
        ("parked: loaded magazines", h["loaded"]),
        ("parked: CPU spares", h["spare"]),
        ("parked: depot", h["depot"]),
        ("free, returnable now", ret),
        ("free, on pages or huge units in use", stuck),
        ("above watermarks", h["above"]),
        ("region metadata", h["meta"]),
        ("magazine area", h["mags"]),
        ("slabs", h["slabs"]),
        ("objects over a region (runs)", h["runs"]),
    ]
    print("\n| where | MiB | of heap |\n|---|---|---|")
    for name, v in rows:
        print(f"| {name} | {v / MiB:.1f} | {v / max(h['heap'], 1):.1%} |")
    print(f"\nsmall classes resident {sum(c['resident'] for c in small) / MiB:.0f} MiB, large {sum(c['resident'] for c in large) / MiB:.0f}; huge-page part committed {huge / MiB:.0f} MiB; back end's retained count {h['retained'] / MiB:.0f} MiB")
    def excess(c):
        return c["resident"] - max(c["req"], 0)
    print(f"\n| class | req MiB | resident | live slots | parked | free returnable | free stuck | above | huge part |\n|---|---|---|---|---|---|---|---|---|")
    for c in sorted(classes, key=excess, reverse=True)[:top]:
        r, s = split(c)
        parked = c["loaded"][0] + c["spare"][0] + c["depot"][0]
        print(
            f"| {c['size']} | {c['req'] / MiB:.1f} | {c['resident'] / MiB:.1f} | {c['live'] / MiB:.1f} | {parked / MiB:.1f} | "
            f"{r / MiB:.1f} | {s / MiB:.1f} | {c['above'] / MiB:.1f} | {c['huge_part'] / MiB:.0f} |"
        )


def categories(h, classes):
    ret = sum(split(c)[0] for c in classes)
    stuck = sum(split(c)[1] for c in classes)
    return {
        "heap": h["heap"],
        "asked for": h["req"],
        "live slots": h["live"],
        "parked": h["loaded"] + h["spare"] + h["depot"],
        "free returnable": ret,
        "free stuck": stuck,
        "above": h["above"],
        "metadata, magazines, slabs": h["meta"] + h["mags"] + h["slabs"],
        "carved": sum(c["carved"] for c in classes),
    }


def occupancy(t):
    return "/".join(str(x) for x in t)


def diff(a, b, top):
    """What changed between breakdowns a and b, each (tag, header, classes)."""
    (ta, ha, ca), (tb, hb, cb) = a, b
    print(f"\n## breakdown {tb} (t={hb['t_ms'] / 1000:.0f} s) less breakdown {ta} (t={ha['t_ms'] / 1000:.0f} s), MiB\n")
    ka, kb = categories(ha, ca), categories(hb, cb)
    print("| | first | second | change |\n|---|---|---|---|")
    for k in ka:
        print(f"| {k} | {ka[k] / MiB:.1f} | {kb[k] / MiB:.1f} | {(kb[k] - ka[k]) / MiB:+.1f} |")
    by_a = {c["size"]: c for c in ca}
    zero = {k: (0,) * 6 if isinstance(v, tuple) and len(v) == 6 else ((0,) * len(v) if isinstance(v, tuple) else 0) for k, v in cb[0].items()} if cb else {}
    rows = []
    for c in cb:
        o = by_a.get(c["size"], dict(zero, size=c["size"]))
        rows.append((c["resident"] - o["resident"], o, c))
    rows.sort(key=lambda r: r[0], reverse=True)
    print(
        "\nClasses by growth of resident memory. Pages and units: resident 4 KiB pages below the watermark, "
        "and resident 2 MiB units of the huge-page part, by the share of their carved bytes not free in the "
        "bitmap: empty/<=25%/<=50%/<=75%/<100%/full.\n"
    )
    print("| class | resident | carved | live slots | parked | free returnable | free stuck | req | pages then | pages now | units then | units now |")
    print("|---|---|---|---|---|---|---|---|---|---|---|---|")
    for d, o, c in rows[:top]:
        ro, rc = split(o), split(c)
        po = o["loaded"][0] + o["spare"][0] + o["depot"][0]
        pc = c["loaded"][0] + c["spare"][0] + c["depot"][0]
        print(
            f"| {c['size']} | {d / MiB:+.1f} | {(c['carved'] - o['carved']) / MiB:+.1f} | {(c['live'] - o['live']) / MiB:+.1f} | "
            f"{(pc - po) / MiB:+.1f} | {(rc[0] - ro[0]) / MiB:+.1f} | {(rc[1] - ro[1]) / MiB:+.1f} | {(c['req'] - o['req']) / MiB:+.1f} | "
            f"{occupancy(o.get('pages', ()))} | {occupancy(c.get('pages', ()))} | {occupancy(o.get('units', ()))} | {occupancy(c.get('units', ()))} |"
        )


def main():
    args = sys.argv[1:]
    if not args:
        sys.exit(__doc__)
    if "--diff" in args:
        want = [int(t) for t in args[args.index("--diff") + 1].split(",")]
        found = {tag: (tag, h, c) for tag, h, c in blocks(args[0], "acct") if tag in want}
        top = int(args[args.index("--classes") + 1]) if "--classes" in args else 12
        diff(found[want[0]], found[want[1]], top)
        return
    tags = None
    if "--tags" in args:
        tags = {int(t) for t in args[args.index("--tags") + 1].split(",")}
    top = int(args[args.index("--classes") + 1]) if "--classes" in args else 12
    label = args[args.index("--label") + 1] if "--label" in args else "acct"
    for tag, h, classes in blocks(args[0], label):
        if tags is None or tag in tags:
            report(tag, h, classes, top)


if __name__ == "__main__":
    main()
