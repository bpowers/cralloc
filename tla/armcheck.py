#!/usr/bin/env python3
"""Litmus tests of what the allocator relies on, under Arm's axiomatic memory model.

The relations are transcribed by hand from herd's aarch64hwreqs.cat and aarch64.cat, for the
explicit accesses and barriers these tests use: no tags, translation, cache maintenance or
exclusives.

    obs = rfe | coe | fre
    dob = addr | data | ctrl;[W] | addr;po;[W] | (addr|data);lmrs;[R]
    aob = rmw | rmw;lmrs;[A|Q]
    bob = po;[dmb.full];po | [R];po;[dmb.ld];po | [W];po;[dmb.st];po;[W]
        | [range(A;amo;L)];po | [L];po;[A] | [A|Q];po | po;[L]
    ob  = (obs | dob | aob | bob)+,  irreflexive
    internal: coherence per location (po-loc | rf | co | fr acyclic), and an RMW's atomicity.

The model's IFB-ob, for ISB and an exception's return, is left out: no test here uses it.

A test lists threads of instructions and asks whether a bad outcome is allowed, by listing every
candidate execution. The first group are litmus tests whose answers are published, there to catch a
mistake in the transcription; the rest are this allocator's (docs/research/memory-ordering.md). Run
with `python3 tla/armcheck.py`: it prints each answer and fails if one is not what the test expects.
herd7 itself is the check to prefer, where it can be installed.
"""
import itertools
import sys

ALLOWED, FORBIDDEN = True, False
FAILED = []


class E:
    def __init__(s, tid, idx, kind, loc=None, val=None, ord="", reg=None):
        s.tid, s.idx, s.kind, s.loc, s.val, s.ord, s.reg = tid, idx, kind, loc, val, ord, reg

    def __repr__(s):
        return f"T{s.tid}.{s.idx}:{s.kind}{s.ord}({s.loc}={s.val})"


def R(loc, reg, ord="", addr=None, ctrl=None):
    return ("R", loc, reg, ord, addr or [], ctrl or [])


def W(loc, val, ord="", data=None, addr=None, ctrl=None):
    return ("W", loc, val, ord, data or [], addr or [], ctrl or [])


def RMW(loc, val, reg, ord="AL"):
    """An atomic: reads into reg and writes val."""
    return ("RMW", loc, val, reg, ord)


def F(kind):
    return ("F", kind)


def closure(rel):
    nxt = {}
    for a, b in rel:
        nxt.setdefault(a, set()).add(b)
    changed = True
    while changed:
        changed = False
        for a in nxt:
            new = set()
            for b in nxt[a]:
                new |= nxt.get(b, set())
            if not new <= nxt[a]:
                nxt[a] |= new
                changed = True
    return nxt


def acyclic(rel):
    c = closure(rel)
    return all(a not in c.get(a, ()) for a in c)


def is_w(e):
    return e.kind == "W"


def is_r(e):
    return e.kind == "R"


def is_m(e):
    return e.kind in ("R", "W")


def external(rel):
    return {(a, b) for a, b in rel if a.tid != b.tid}


def run(name, threads, ask, expect):
    """Prints whether an outcome that `ask` picks is allowed, and records it if that is not `expect`."""
    events, po = [], set()
    regs = {}
    rmw, addr, data, ctrl = set(), set(), set(), set()
    for t, code in enumerate(threads):
        mine = []
        for inst in code:
            if inst[0] == "R":
                _, loc, reg, ord, adeps, cdeps = inst
                e = E(t, len(mine), "R", loc, None, ord, reg)
                mine.append(e)
                regs[(t, reg)] = e
                addr |= {(regs[(t, d)], e) for d in adeps}
                ctrl |= {(regs[(t, d)], e) for d in cdeps}
            elif inst[0] == "W":
                _, loc, val, ord, ddeps, adeps, cdeps = inst
                e = E(t, len(mine), "W", loc, val, ord)
                mine.append(e)
                data |= {(regs[(t, d)], e) for d in ddeps}
                addr |= {(regs[(t, d)], e) for d in adeps}
                ctrl |= {(regs[(t, d)], e) for d in cdeps}
            elif inst[0] == "RMW":
                _, loc, val, reg, ord = inst
                acquire = "A" if "A" in ord else ("Q" if "Q" in ord else "")
                r = E(t, len(mine), "R", loc, None, acquire, reg)
                w = E(t, len(mine) + 1, "W", loc, val, "L" if "L" in ord else "")
                mine += [r, w]
                regs[(t, reg)] = r
                rmw.add((r, w))
            else:
                mine.append(E(t, len(mine), inst[1]))
        events += mine
        po |= set(itertools.combinations(mine, 2))
    reads = [e for e in events if is_r(e)]
    writes = [e for e in events if is_w(e)]
    locs = sorted({e.loc for e in events if e.loc})
    initial = {l: E(-1, 0, "W", l, 0) for l in locs}
    by_loc = {l: [w for w in writes if w.loc == l] for l in locs}
    fences = [e for e in events if e.kind in ("dmb", "dmb.ld", "dmb.st")]
    allowed = set()

    for orders in itertools.product(*[list(itertools.permutations(by_loc[l])) for l in locs]):
        co = {l: [initial[l]] + list(order) for l, order in zip(locs, orders)}
        coset = set()
        for seq in co.values():
            coset |= set(itertools.combinations(seq, 2))
        for sources in itertools.product(*[[initial[r.loc]] + by_loc[r.loc] for r in reads]):
            rf = dict(zip(reads, sources))
            fr = set()
            for r, w in rf.items():
                seq = co[r.loc]
                fr |= {(r, later) for later in seq[seq.index(w) + 1 :]}
            rfs = {(w, r) for r, w in rf.items()}
            # Internal: coherence per location.
            poloc = {(a, b) for a, b in po if a.loc and a.loc == b.loc}
            if not acyclic(poloc | rfs | coset | fr):
                continue
            # Atomicity: no write between the source of an RMW's read and its write.
            if any(co[r.loc].index(w) != co[r.loc].index(rf[r]) + 1 for r, w in rmw):
                continue
            obs = external(rfs) | external(coset) | external(fr)
            dob = set(addr) | set(data) | {(a, b) for a, b in ctrl if is_w(b)}
            for a, b in addr:
                dob |= {(a, c) for c in events if (b, c) in po and is_w(c)}
            # lmrs: a read's most recent local store to its location, which it reads from.
            lmrs = {(w, r) for r, w in rf.items() if w.tid == r.tid and (w, r) in po}
            for a, b in addr | data:
                dob |= {(a, r) for w, r in lmrs if b is w}
            aob = set(rmw)
            for _, w in rmw:
                aob |= {(w, r) for w2, r in lmrs if w2 is w and r.ord in ("A", "Q")}
            bob = set()
            for f in fences:
                for a in events:
                    for b in events:
                        if (a, f) in po and (f, b) in po and is_m(a) and is_m(b):
                            if f.kind == "dmb":
                                bob.add((a, b))
                            elif f.kind == "dmb.ld" and is_r(a):
                                bob.add((a, b))
                            elif f.kind == "dmb.st" and is_w(a) and is_w(b):
                                bob.add((a, b))
            for a, b in po:
                if not (is_m(a) and is_m(b)):
                    continue
                if is_w(a) and a.ord == "L" and any(w is a and r.ord == "A" for r, w in rmw):
                    bob.add((a, b))
                if is_w(a) and a.ord == "L" and is_r(b) and b.ord == "A":
                    bob.add((a, b))
                if is_r(a) and a.ord in ("A", "Q"):
                    bob.add((a, b))
                if is_w(b) and b.ord == "L":
                    bob.add((a, b))
            if not acyclic(obs | dob | aob | bob):
                continue

            def value(w):
                # A write's value, or for an RMW's write a function of what it read.
                if callable(w.val):
                    read = next(r for r, w2 in rmw if w2 is w)
                    return w.val(value(rf[read]))
                return w.val

            outcome = {f"{t}:{reg}": value(rf[e]) for (t, reg), e in regs.items()}
            outcome |= {l: value(co[l][-1]) for l in locs}
            allowed.add(tuple(sorted(outcome.items())))
    hit = any(ask(dict(o)) for o in allowed)
    print(f"{name}: {'ALLOWED' if hit else 'forbidden'}")
    if hit != expect:
        FAILED.append(name)


def both_zero(o):
    return o["0:r"] == 0 and o["1:r"] == 0


def published():
    """Tests from the literature, each with the answer herd's catalogue gives."""
    mp = lambda o: o["1:a"] == 1 and o["1:b"] == 0
    run("MP", [[W("x", 1), W("y", 1)], [R("y", "a"), R("x", "b")]], mp, ALLOWED)
    run("MP+dmb+addr",
        [[W("x", 1), F("dmb"), W("y", 1)], [R("y", "a"), R("x", "b", addr=["a"])]], mp, FORBIDDEN)
    run("MP+po+addr",
        [[W("x", 1), W("y", 1)], [R("y", "a"), R("x", "b", addr=["a"])]], mp, ALLOWED)
    run("MP+stlr+ldapr",
        [[W("x", 1), W("y", 1, "L")], [R("y", "a", "Q"), R("x", "b")]], mp, FORBIDDEN)
    run("SB", [[W("x", 1), R("y", "r")], [W("y", 1), R("x", "r")]], both_zero, ALLOWED)
    run("SB+dmbs",
        [[W("x", 1), F("dmb"), R("y", "r")], [W("y", 1), F("dmb"), R("x", "r")]],
        both_zero, FORBIDDEN)
    both_one = lambda o: o["0:r"] == 1 and o["1:r"] == 1
    run("LB", [[R("x", "r"), W("y", 1)], [R("y", "r"), W("x", 1)]], both_one, ALLOWED)
    run("LB+datas",
        [[R("x", "r"), W("y", 1, data=["r"])], [R("y", "r"), W("x", 1, data=["r"])]],
        both_one, FORBIDDEN)
    # Other-multi-copy atomicity: a store seen by one other thread is seen by them all.
    run("WRC+addrs",
        [[W("x", 1)], [R("x", "r"), W("y", 1, addr=["r"])], [R("y", "a"), R("x", "b", addr=["a"])]],
        lambda o: o["1:r"] == 1 and o["2:a"] == 1 and o["2:b"] == 0, FORBIDDEN)
    run("IRIW+addrs",
        [[W("x", 1)], [W("y", 1)],
         [R("x", "a"), R("y", "b", addr=["a"])], [R("y", "c"), R("x", "d", addr=["c"])]],
        lambda o: o["2:a"] == 1 and o["2:b"] == 0 and o["3:c"] == 1 and o["3:d"] == 0, FORBIDDEN)
    run("2+2W", [[W("x", 1), W("y", 2)], [W("y", 1), W("x", 2)]],
        lambda o: o["x"] == 1 and o["y"] == 1, ALLOWED)
    run("2+2W+dmbs",
        [[W("x", 1), F("dmb"), W("y", 2)], [W("y", 1), F("dmb"), W("x", 2)]],
        lambda o: o["x"] == 1 and o["y"] == 1, FORBIDDEN)
    run("R+dmbs",
        [[W("x", 1), F("dmb"), W("y", 1)], [W("y", 2), F("dmb"), R("x", "r")]],
        lambda o: o["y"] == 2 and o["1:r"] == 0, FORBIDDEN)
    run("S+dmb+data",
        [[W("x", 2), F("dmb"), W("y", 1)], [R("y", "r"), W("x", 1, data=["r"])]],
        lambda o: o["1:r"] == 1 and o["x"] == 2, FORBIDDEN)


def ours():
    """What the allocator rests on: each outcome the model must rule out, and beside it the weaker
    form that shows the enumerator can see that outcome."""
    # Store buffering with SeqCst on both sides, as rustc maps it: atomics with both semantics and
    # ldar. The summary bits' protocol.
    run("SB: ldsetal;ldar (SeqCst RMW and SeqCst load, the summary bits)",
        [[RMW("x", 1, "a"), R("y", "r", "A")], [RMW("y", 1, "a"), R("x", "r", "A")]],
        both_zero, FORBIDDEN)
    run("SB: ldsetl;ldapr (release RMW, acquire load: what SeqCst must not be weakened to)",
        [[RMW("x", 1, "a", "L"), R("y", "r", "Q")], [RMW("y", 1, "a", "L"), R("x", "r", "Q")]],
        both_zero, ALLOWED)
    run("SB: ldsetal;ldr (an LSE atomic before a plain load)",
        [[RMW("x", 1, "a"), R("y", "r")], [RMW("y", 1, "a"), R("x", "r")]],
        both_zero, FORBIDDEN)
    run("SB: stlr;ldapr",
        [[W("x", 1, "L"), R("y", "r", "Q")], [W("y", 1, "L"), R("x", "r", "Q")]],
        both_zero, ALLOWED)
    run("SB: plain store;ldar (a failed compare-and-swap writes nothing, so orders no store)",
        [[W("x", 1), R("y", "r", "A")], [W("y", 1), R("x", "r", "A")]],
        both_zero, ALLOWED)
    # The clocked pass's start: casl, stlr, ldar against the thread's mark and its read.
    run("sleep: casl;stlr;ldar against casal;ldar",
        [[RMW("decay", 1, "a", "L"), W("pass", 1, "L"), R("thread", "r", "A")],
         [RMW("thread", 1, "a"), R("decay", "r", "A")]],
        both_zero, FORBIDDEN)
    # The parking protocol: a slow path's plain store and acquire load, against the thread's
    # atomic, a fence's barrier on the slow path's CPU between any two of its instructions, and
    # its look. Each placement of the kernel's barrier in the slow path's program.
    for at in (0, 1, 2):
        s = [W("slab", 1), R("word", "r", "A")]
        s.insert(at, F("dmb"))
        # The kernel's handshake: the slow path's barrier comes after the thread's first barrier
        # and before its last. Modelled with a word the IPI's send and its completion use.
        s.insert(at, R("ipi", "i"))
        s.insert(at + 2, W("ack", 1))
        run(f"park: barrier at {at} in the slow path",
            [s, [RMW("word", 1, "a"), F("dmb"), W("ipi", 1), F("dmb"), R("ack", "k"), F("dmb"),
                 R("slab", "r")]],
            lambda o: o["0:r"] == 0 and o["1:r"] == 0 and o["0:i"] == 1 and o["1:k"] == 1,
            FORBIDDEN)
    # A slab word read without protection, and the mark that admits its magazine. The installer
    # made the magazine: an atomic on the mark, then its acquire read of the mark, then a plain
    # store of the word. The reader: acquire loads of the word and the mark.
    run("slab word: ldumaxal, ldapr, str against ldapr, ldapr",
        [[RMW("mark", 1, "a"), R("mark", "m", "Q"), W("word", 1, ctrl=["m"])],
         [R("word", "r", "Q"), R("mark", "s", "Q")]],
        lambda o: o["1:r"] == 1 and o["1:s"] == 0, FORBIDDEN)
    run("slab word: another thread's mark, then ldapr, str, against ldapr, ldapr",
        [[RMW("mark", 1, "a")],
         [R("mark", "m", "Q"), W("word", 1, ctrl=["m"])],
         [R("word", "r", "Q"), R("mark", "s", "Q")]],
        lambda o: o["1:m"] == 1 and o["2:r"] == 1 and o["2:s"] == 0, FORBIDDEN)
    run("slab word: reader without acquire (what the Acquire is for)",
        [[RMW("mark", 1, "a"), R("mark", "m", "Q"), W("word", 1, ctrl=["m"])],
         [R("word", "r"), R("mark", "s", "Q")]],
        lambda o: o["1:r"] == 1 and o["1:s"] == 0, ALLOWED)
    # The second look before a refusal: a plain read of the word, a full fence, the mark again.
    run("slab word: plain ldr of the word, dmb, ldapr of the mark",
        [[RMW("mark", 1, "a"), R("mark", "m", "Q"), W("word", 1, ctrl=["m"])],
         [R("word", "r"), F("dmb"), R("mark", "s", "Q")]],
        lambda o: o["1:r"] == 1 and o["1:s"] == 0, FORBIDDEN)
    # The depot stack: a push's plain slot and release of the head, a pop's acquire of the head and
    # read of the link, and a read of what the pusher wrote through pops between.
    run("stack: push casal, pop casal, a third acquires the head: the slot is there",
        [[W("slot", 1), RMW("head", 1, "a", "AL")],
         [R("head", "h", "Q"), RMW("head", 2, "b", "AL")],
         [R("head", "c", "Q"), R("slot", "s")]],
        lambda o: o["1:h"] == 1 and o["1:b"] == 1 and o["2:c"] == 2 and o["2:s"] == 0, FORBIDDEN)
    run("stack: as above, the pop between relaxed: Arm's aob still orders it",
        [[W("slot", 1), RMW("head", 1, "a", "AL")],
         [RMW("head", 2, "b", "")],
         [R("head", "c", "Q"), R("slot", "s")]],
        lambda o: o["1:b"] == 1 and o["2:c"] == 2 and o["2:s"] == 0, FORBIDDEN)
    run("stack: a push with no release",
        [[W("slot", 1), RMW("head", 1, "a", "A")],
         [R("head", "c", "Q"), R("slot", "s")]],
        lambda o: o["1:c"] == 1 and o["1:s"] == 0, ALLOWED)
    run("stack: a pop's first read of the head acquires: its link is the pusher's",
        [[W("link", 1), RMW("head", 1, "a", "L")],
         [R("head", "h", "Q"), R("link", "l"), RMW("head", 2, "b", "A")]],
        lambda o: o["1:h"] == 1 and o["1:l"] == 0, FORBIDDEN)
    run("stack: that read relaxed: a stale link, and a compare-and-swap that succeeds",
        [[W("link", 1), RMW("head", 1, "a", "L")],
         [R("head", "h"), R("link", "l"), RMW("head", 2, "b", "A")]],
        lambda o: o["1:h"] == 1 and o["1:l"] == 0 and o["1:b"] == 1, ALLOWED)
    run("stack: a link read after a failed compare-and-swap's acquire",
        [[W("link", 1), RMW("head", 1, "a", "L")],
         [RMW("head", 9, "f", "A"), R("link", "l")]],
        lambda o: o["1:f"] == 1 and o["1:l"] == 0, FORBIDDEN)
    # What a context switch stands in for: a thread's plain stores, the kernel's barrier, the next
    # thread on that CPU, whose push releases, then a pop elsewhere.
    run("chain: plain stores, context switch, next thread's release, pop elsewhere",
        [[W("bytes", 1), W("slot", 1), F("dmb"), W("mag", 1, "L")],
         [R("mag", "m", "Q"), R("bytes", "b"), R("slot", "s")]],
        lambda o: o["1:m"] == 1 and (o["1:b"] == 0 or o["1:s"] == 0), FORBIDDEN)
    # A pointer passed by a relaxed store: the thread that frees it reads what the address says.
    run("relaxed hand-off: tag CAS, ldar, str g against ldr g, addr ldapr tag",
        [[RMW("tag", 1, "a"), R("other", "o", "A"), W("g", 1)],
         [R("g", "p"), R("tag", "t", "Q", addr=["p"])]],
        lambda o: o["1:p"] == 1 and o["1:t"] == 0, FORBIDDEN)
    run("relaxed hand-off: the heap word has no address dependency",
        [[W("heap", 1, "L"), R("other", "o", "A"), W("g", 1)],
         [R("g", "p"), R("heap", "h", "Q")]],
        lambda o: o["1:p"] == 1 and o["1:h"] == 0, ALLOWED)
    # Message passing through a store a routine makes: what a drain's fence stands in for.
    run("drain: plain slot and word stores, swpal then barrier then read",
        [[W("slot", 1), W("word", 1), F("dmb"), W("ack", 1)],
         [RMW("word", 2, "old"), R("ack", "k", "A"), R("slot", "s")]],
        lambda o: o["1:old"] == 1 and o["1:k"] == 1 and o["1:s"] == 0, FORBIDDEN)
    run("drain: without the barrier the slot can be stale",
        [[W("slot", 1), W("word", 1)],
         [RMW("word", 2, "old"), R("slot", "s")]],
        lambda o: o["1:old"] == 1 and o["1:s"] == 0, ALLOWED)
    # The fence with no interrupt, where the caller reads a `rq->curr` of another address space.
    # The thread left: its stores, the scheduler's barrier and its store of `rq->curr`, against the
    # exchange, the caller's barrier, its read of `rq->curr`, its barrier on the way out, and a read.
    run("fence, thread gone: str, str, dmb, str curr against swpal, dmb, ldr curr, dmb, ldr",
        [[W("slot", 1), W("word", 1), F("dmb"), W("curr", 1)],
         [RMW("word", 2, "old"), F("dmb"), R("curr", "c"), F("dmb"), R("slot", "s")]],
        lambda o: o["1:old"] == 1 and o["1:c"] == 1 and o["1:s"] == 0, FORBIDDEN)
    # The thread is about to come back: its store of `rq->curr`, the `dsb` in `__switch_to`, and a
    # read of the slab word, against the marker, the caller's barrier and a read that misses it.
    run("fence, thread returning: str curr, dmb, ldr word against swpal, dmb, ldr curr",
        [[W("curr", 1), F("dmb"), R("word", "r")],
         [RMW("word", 2, "old"), F("dmb"), R("curr", "r")]],
        both_zero, FORBIDDEN)
    # Each barrier those rest on, left out, and the outcome comes back.
    park = lambda o: o["0:r"] == 0 and o["1:r"] == 0 and o["0:i"] == 1 and o["1:k"] == 1
    run("park, control: no barrier in the slow path",
        [[R("ipi", "i"), W("slab", 1), R("word", "r", "A"), W("ack", 1)],
         [RMW("word", 1, "a"), F("dmb"), W("ipi", 1), F("dmb"), R("ack", "k"), F("dmb"),
          R("slab", "r")]],
        park, ALLOWED)
    run("park, control: no barriers in the parker",
        [[R("ipi", "i"), F("dmb"), W("ack", 1), W("slab", 1), R("word", "r", "A")],
         [RMW("word", 1, "a", "L"), W("ipi", 1), R("ack", "k"), R("slab", "r")]],
        park, ALLOWED)
    run("fence, thread gone, control: no scheduler barrier",
        [[W("slot", 1), W("word", 1), W("curr", 1)],
         [RMW("word", 2, "old"), F("dmb"), R("curr", "c"), F("dmb"), R("slot", "s")]],
        lambda o: o["1:old"] == 1 and o["1:c"] == 1 and o["1:s"] == 0, ALLOWED)
    run("fence, thread gone, control: no barrier after the caller's read of rq->curr",
        [[W("slot", 1), W("word", 1), F("dmb"), W("curr", 1)],
         [RMW("word", 2, "old"), F("dmb"), R("curr", "c"), R("slot", "s")]],
        lambda o: o["1:old"] == 1 and o["1:c"] == 1 and o["1:s"] == 0, ALLOWED)
    run("fence, thread returning, control: no dsb after the store of rq->curr",
        [[W("curr", 1), R("word", "r")],
         [RMW("word", 2, "old"), F("dmb"), R("curr", "r")]],
        both_zero, ALLOWED)
    run("sleep, control: ldapr in place of ldar",
        [[RMW("decay", 1, "a", "L"), W("pass", 1, "L"), R("thread", "r", "Q")],
         [RMW("thread", 1, "a"), R("decay", "r", "A")]],
        both_zero, ALLOWED)
    run("drain, control: the handshake not acquired, and a plain exchange",
        [[W("slot", 1), W("word", 1), F("dmb"), W("ack", 1)],
         [RMW("word", 2, "old", ""), R("ack", "k"), R("slot", "s")]],
        lambda o: o["1:old"] == 1 and o["1:k"] == 1 and o["1:s"] == 0, ALLOWED)
    # The shortcut: an exited thread's stores, its full-barrier decrement, the caller's read of the
    # count, and then its load.
    run("shortcut: decrement, then ldr of the count, then a load, no barrier",
        [[W("slot", 1), RMW("users", 1, "a")], [R("users", "u"), R("slot", "s")]],
        lambda o: o["1:u"] == 1 and o["1:s"] == 0, ALLOWED)
    run("shortcut: with the acquire fence",
        [[W("slot", 1), RMW("users", 1, "a")], [R("users", "u"), F("dmb.ld"), R("slot", "s")]],
        lambda o: o["1:u"] == 1 and o["1:s"] == 0, FORBIDDEN)


if __name__ == "__main__":
    published()
    ours()
    if FAILED:
        sys.exit(f"not as expected: {FAILED}")
