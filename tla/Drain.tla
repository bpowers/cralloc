------------------------------- MODULE Drain -------------------------------
(* Draining an idle cid's slab from another thread, and repairing a forked
   child in which a drain was under way.

   Every routine is several steps and commits with its last, because the
   drain's correctness is about sequences in flight. Each cid has one slab
   word (one class) and one spare stack. Magazines carry a fill level and no
   object identities; `truth` is a ghost that records each magazine's real
   level, so a drainer that takes a stale word is caught.

   Protocol:
     "v1"     the first design: one word at a time, one marker value, no claim.
     "claim"  the current design: claim the slab's drain flag, exchange the
              slab word and the spare head, one fence, re-read each, release.
     "gen"    proposed: as "claim", but the flag and the marker carry the
              process's fork generation, older generations count as absent,
              and nothing is swept after a fork.

   ForkMode:
     "none"   no fork.
     "idle"   one fork, by a thread that is outside the allocator.
     "any"    one fork, by a thread at any step (a signal handler that
              interrupted the allocator called fork).
   After a fork only the forking thread keeps its state. The others become
   fresh threads, which stands for threads the child creates later.

   With "claim" the child repairs by sweeping, as the design says: the first
   slow path to see the wiped page clears every marker and drain flag.
   HoldDrains = TRUE is a proposed restriction: no drain starts until the
   sweep has finished. *)
EXTENDS Naturals, FiniteSets, TLC

CONSTANTS
    Threads,
    Mutators,       \* threads that allocate and free
    Drainers,       \* threads that drain
    NCids,
    NMags,
    Cap,            \* objects per magazine
    Protocol,
    ForkMode,
    HoldDrains,
    ZeroOnlyIfMag,  \* TRUE: step 4 writes zero only if the old value named a magazine
    FenceCanFail,   \* TRUE: membarrier may fail, and the drain backs out
    InPlace         \* TRUE: pop and push commit by adjusting the word in place

Cids == 1..NCids
Mags == 1..NMags

\* A word is zero, a marker (k # 0), or names magazine m at fill level s.
\* Spare-stack heads and magazine links are words with s = 0.
Zero       == [m |-> 0, s |-> 0, k |-> 0]
Marker(g)  == [m |-> 0, s |-> 0, k |-> g]
Word(m, s) == [m |-> m, s |-> s, k |-> 0]
Head(m)    == Word(m, 0)
IsMark(w)  == w.k # 0
IsMag(w)   == w.m # 0

VARIABLES
    slab,    \* slab[c]: the slab word
    spare,   \* spare[c]: head of the spare stack
    flag,    \* flag[c]: the drain claim
    link,    \* link[m]: slot 0 of magazine m
    lvl,     \* lvl[m]: fill level recorded in an unloaded magazine
    depot,   \* magazines in the depot (Depot.tla models the stack itself)
    truth,   \* ghost: the real fill level of each magazine
    th,      \* per-thread private state
    fence,   \* fence[t]: threads whose sequences t's membarrier still waits for
    gen,     \* the process's fork generation
    wipe,    \* the wiped page: 0 fresh child, 1 sweep under way, 2 nothing to do
    forked,
    lost     \* ghost: magazines no survivor could reach right after the fork

vars == <<slab, spare, flag, link, lvl, depot, truth, th, fence, gen, wipe, forked, lost>>

Fresh == [pc |-> "idle", op |-> "none", rc |-> 0, rw |-> Zero, rn |-> Zero,
          hw |-> Zero,              \* magazine held privately
          dc |-> 0, tg |-> {},      \* drain: the cid, and which words
          g |-> 0,                  \* drain: the claim and marker value
          old |-> Zero, osp |-> Zero,   \* drain: exchanged values, not yet owned
          w1 |-> Zero, w2 |-> Zero,     \* drain: values it now owns
          ret |-> "idle"]

Init ==
    /\ slab = [c \in Cids |-> IF c = 1 THEN Word(1, Cap) ELSE Zero]
    /\ spare = [c \in Cids |-> Zero]
    /\ flag = [c \in Cids |-> 0]
    /\ link = [m \in Mags |-> Zero]
    /\ lvl = [m \in Mags |-> IF m = NMags THEN 0 ELSE Cap]
    /\ depot = Mags \ {1}
    /\ truth = [m \in Mags |-> IF m = NMags THEN 0 ELSE Cap]
    /\ th = [t \in Threads |-> Fresh]
    /\ fence = [t \in Threads |-> {}]
    /\ gen = 1
    /\ wipe = 2
    /\ forked = FALSE
    /\ lost = {}

\* A sequence's first step here reads the cid and the word together; Rseq.tla
\* checks the cid read on its own. A thread is in flight once it has read the
\* word and until it commits, leaves or is restarted.
InSeq(l) == l \in {"pop_commit", "push_commit", "ins_commit",
                   "sp_link", "sp_commit", "spp_store", "spp_commit"}

SeqStart(l) ==
    CASE l = "pop_commit"                   -> "pop"
      [] l = "push_commit"                  -> "push"
      [] l = "ins_commit"                   -> "ins"
      [] l \in {"sp_link", "sp_commit"}     -> "sp"
      [] l \in {"spp_store", "spp_commit"}  -> "spp"
      [] OTHER -> l

(* Cids. A thread's cid matters only while it is inside a sequence: outside
   one, the OS may move it freely and nothing depends on where it runs. So
   the model keeps no cid for a thread outside a sequence. A sequence starts
   on any cid that no sequence in flight is using, which is the OS's promise
   that two running threads never share a cid; for another thread to start a
   sequence on that cid the OS must first stop the one in flight, which is
   Abort below. A thread needs a cid to run at all, so it cannot take a step
   while NCids other threads are inside sequences. Rseq.tla models cids
   without this shortcut. *)
Flying(t) == InSeq(th[t].pc)
FreeCids(t) == {c \in Cids : \A u \in Threads \ {t} : Flying(u) => th[u].rc # c}
Running(t) == Flying(t) \/ Cardinality({u \in Threads \ {t} : Flying(u)}) < NCids

\* What an in-place commit leaves when the word under it is not a magazine.
Corrupt == [m |-> 0, s |-> Cap + 1, k |-> 0]

Restarted(T) == [T EXCEPT !.pc = SeqStart(T.pc), !.rc = 0, !.rw = Zero, !.rn = Zero]

Set(t, T) == th' = [th EXCEPT ![t] = T]

Suitable(op, s) == IF op = "alloc" THEN s > 0 ELSE s < Cap

----------------------------------------------------------------------------
(* Allocation and free *)

Start(t) ==
    /\ t \in Mutators
    /\ th[t].pc = "idle"
    /\ \/ Set(t, [th[t] EXCEPT !.pc = "pop", !.op = "alloc"])
       \/ Set(t, [th[t] EXCEPT !.pc = "push", !.op = "free"])
    /\ UNCHANGED <<slab, spare, flag, link, lvl, depot, truth, gen, wipe, forked, lost>>

\* Pop and push read the word and take the slow path on zero, the marker, or
\* an empty (full) magazine.
FastLoad(t) == \E c \in FreeCids(t) :
    /\ th[t].pc \in {"pop", "push"}
    /\ LET T == th[t]
           w == slab[c] IN
        IF IsMag(w) /\ Suitable(T.op, w.s)
        THEN Set(t, [T EXCEPT !.rc = c, !.rw = w,
                              !.pc = IF T.op = "alloc" THEN "pop_commit" ELSE "push_commit"])
        ELSE Set(t, [T EXCEPT !.pc = "get"])
    /\ UNCHANGED <<slab, spare, flag, link, lvl, depot, truth, gen, wipe, forked, lost>>

FastCommit(t) ==
    /\ th[t].pc \in {"pop_commit", "push_commit"}
    /\ LET T == th[t]
           \* The design commits a value computed from the word it read. The
           \* in-place variant adjusts whatever the word holds now.
           base == IF InPlace THEN slab[T.rc] ELSE T.rw
           m == T.rw.m
           s == IF T.op = "alloc" THEN base.s - 1 ELSE base.s + 1 IN
        /\ slab' = [slab EXCEPT ![T.rc] = IF IsMag(base) THEN Word(base.m, s) ELSE Corrupt]
        /\ truth' = [truth EXCEPT ![m] = IF T.op = "alloc" THEN @ - 1 ELSE @ + 1]
    /\ Set(t, Fresh)
    /\ UNCHANGED <<spare, flag, link, lvl, depot, gen, wipe, forked, lost>>

\* The slow path gets a magazine from the depot or the cid's spares, or
\* fails the request.
Get(t) ==
    /\ th[t].pc = "get"
    /\ wipe # 0
    /\ LET T == th[t] IN
        \/ \E m \in depot :
            /\ Suitable(T.op, lvl[m])
            /\ depot' = depot \ {m}
            /\ Set(t, [T EXCEPT !.hw = Word(m, lvl[m]), !.pc = "ins"])
        \/ /\ Set(t, [T EXCEPT !.pc = "sp"])
           /\ UNCHANGED depot
        \/ /\ Set(t, Fresh)
           /\ UNCHANGED depot
    /\ UNCHANGED <<slab, spare, flag, link, lvl, truth, gen, wipe, forked, lost>>

\* Spare pop: head, link, commit. The marker reads as an empty stack.
SpLoad(t) == \E c \in FreeCids(t) :
    /\ th[t].pc = "sp"
    /\ LET T == th[t]
           h == spare[c] IN
        IF IsMag(h)
        THEN Set(t, [T EXCEPT !.rc = c, !.rw = h, !.pc = "sp_link"])
        ELSE Set(t, Fresh)
    /\ UNCHANGED <<slab, spare, flag, link, lvl, depot, truth, gen, wipe, forked, lost>>

SpLink(t) ==
    /\ th[t].pc = "sp_link"
    /\ Set(t, [th[t] EXCEPT !.rn = link[th[t].rw.m], !.pc = "sp_commit"])
    /\ UNCHANGED <<slab, spare, flag, link, lvl, depot, truth, gen, wipe, forked, lost>>

SpCommit(t) ==
    /\ th[t].pc = "sp_commit"
    /\ LET T == th[t]
           m == T.rw.m IN
        /\ spare' = [spare EXCEPT ![T.rc] = T.rn]
        /\ Set(t, [T EXCEPT !.hw = Word(m, lvl[m]), !.rc = 0, !.rw = Zero, !.rn = Zero,
                            !.pc = IF Suitable(T.op, lvl[m]) THEN "ins" ELSE "to_depot"])
    /\ UNCHANGED <<slab, flag, link, lvl, depot, truth, gen, wipe, forked, lost>>

\* Install: store the held word only over zero or an empty (full) magazine.
\* On the marker, serve the request from the held magazine and send it to
\* the depot.
InsLoad(t) == \E c \in FreeCids(t) :
    /\ th[t].pc = "ins"
    /\ LET T == th[t]
           w == slab[c]
           d == IF T.op = "alloc" THEN T.hw.s - 1 ELSE T.hw.s + 1 IN
        IF IsMark(w)
        THEN /\ Set(t, [T EXCEPT !.hw = Word(T.hw.m, d), !.pc = "to_depot"])
             /\ truth' = [truth EXCEPT ![T.hw.m] = d]
        ELSE /\ IF w = Zero \/ ~Suitable(T.op, w.s)
                THEN Set(t, [T EXCEPT !.rc = c, !.rw = w, !.pc = "ins_commit"])
                ELSE Set(t, [T EXCEPT !.pc = "put"])
             /\ UNCHANGED truth
    /\ UNCHANGED <<slab, spare, flag, link, lvl, depot, gen, wipe, forked, lost>>

InsCommit(t) ==
    /\ th[t].pc = "ins_commit"
    /\ LET T == th[t] IN
        /\ slab' = [slab EXCEPT ![T.rc] = T.hw]
        /\ Set(t, [T EXCEPT !.hw = T.rw, !.rc = 0, !.rw = Zero, !.pc = "put"])
    /\ UNCHANGED <<spare, flag, link, lvl, depot, truth, gen, wipe, forked, lost>>

\* Retire the displaced magazine, or put back the one that was not installed:
\* to the cid's spares or to the depot.
Put(t) ==
    /\ th[t].pc = "put"
    /\ LET T == th[t] IN
        IF T.hw = Zero THEN Set(t, Fresh)
        ELSE \/ Set(t, [T EXCEPT !.pc = "to_depot"])
             \/ Set(t, [T EXCEPT !.pc = "spp"])
    /\ UNCHANGED <<slab, spare, flag, link, lvl, depot, truth, gen, wipe, forked, lost>>

\* Spare push never pushes onto the marker.
SppLoad(t) == \E c \in FreeCids(t) :
    /\ th[t].pc = "spp"
    /\ LET T == th[t]
           h == spare[c] IN
        IF IsMark(h)
        THEN Set(t, [T EXCEPT !.pc = "to_depot"])
        ELSE Set(t, [T EXCEPT !.rc = c, !.rw = h, !.pc = "spp_store"])
    /\ UNCHANGED <<slab, spare, flag, link, lvl, depot, truth, gen, wipe, forked, lost>>

\* Writes only the private magazine.
SppStore(t) ==
    /\ th[t].pc = "spp_store"
    /\ LET T == th[t] IN
        /\ link' = [link EXCEPT ![T.hw.m] = T.rw]
        /\ lvl' = [lvl EXCEPT ![T.hw.m] = T.hw.s]
        /\ Set(t, [T EXCEPT !.pc = "spp_commit"])
    /\ UNCHANGED <<slab, spare, flag, depot, truth, gen, wipe, forked, lost>>

SppCommit(t) ==
    /\ th[t].pc = "spp_commit"
    /\ spare' = [spare EXCEPT ![th[t].rc] = Head(th[t].hw.m)]
    /\ Set(t, Fresh)
    /\ UNCHANGED <<slab, flag, link, lvl, depot, truth, gen, wipe, forked, lost>>

ToDepot(t) ==
    /\ th[t].pc = "to_depot"
    /\ LET T == th[t] IN
        /\ depot' = depot \cup {T.hw.m}
        /\ lvl' = [lvl EXCEPT ![T.hw.m] = T.hw.s]
    /\ Set(t, Fresh)
    /\ UNCHANGED <<slab, spare, flag, link, truth, gen, wipe, forked, lost>>

----------------------------------------------------------------------------
(* The drain *)

RECURSIVE Chain(_, _)
\* Magazines on the stack that starts at head word h. 0 marks a loop.
Chain(h, acc) ==
    IF ~IsMag(h) THEN acc
    ELSE IF h.m \in acc THEN acc \cup {0}
    ELSE Chain(link[h.m], acc \cup {h.m})

Claimable(f) == f = 0 \/ (Protocol = "gen" /\ f < gen)
Token == IF Protocol = "gen" THEN gen ELSE 1

AfterX1(T) == IF "spare" \in T.tg THEN "d_x2" ELSE "d_fence"
AfterR1(T) == IF "spare" \in T.tg THEN "d_r2" ELSE "d_send"

\* Step 1: claim the slab. "v1" has no claim and drains one word.
DStart(t) ==
    /\ t \in Drainers
    /\ th[t].pc = "idle"
    /\ wipe # 0
    /\ HoldDrains => wipe = 2
    /\ \E c \in Cids :
        IF Protocol = "v1"
        THEN /\ \E w \in {"slab", "spare"} :
                    Set(t, [th[t] EXCEPT !.dc = c, !.tg = {w}, !.g = 1,
                                         !.pc = IF w = "slab" THEN "d_x1" ELSE "d_x2"])
             /\ UNCHANGED flag
        ELSE /\ Claimable(flag[c])
             /\ flag' = [flag EXCEPT ![c] = Token]
             /\ Set(t, [th[t] EXCEPT !.dc = c, !.tg = {"slab", "spare"}, !.g = Token,
                                     !.pc = "d_x1"])
    /\ UNCHANGED <<slab, spare, link, lvl, depot, truth, gen, wipe, forked, lost>>

\* Step 2: exchange each word with the marker.
DX1(t) ==
    /\ th[t].pc = "d_x1"
    /\ LET T == th[t]
           w == slab[T.dc] IN
        /\ slab' = [slab EXCEPT ![T.dc] = Marker(T.g)]
        /\ IF Protocol = "v1" /\ IsMark(w)
           THEN Set(t, Fresh)
           ELSE Set(t, [T EXCEPT !.old = w, !.pc = AfterX1(T)])
    /\ UNCHANGED <<spare, flag, link, lvl, depot, truth, gen, wipe, forked, lost>>

DX2(t) ==
    /\ th[t].pc = "d_x2"
    /\ LET T == th[t]
           w == spare[T.dc] IN
        /\ spare' = [spare EXCEPT ![T.dc] = Marker(T.g)]
        /\ IF Protocol = "v1" /\ IsMark(w)
           THEN Set(t, Fresh)
           ELSE Set(t, [T EXCEPT !.osp = w, !.pc = "d_fence"])
    /\ UNCHANGED <<slab, flag, link, lvl, depot, truth, gen, wipe, forked, lost>>

\* Step 3: membarrier. It returns once every sequence in flight at the call
\* has completed or been aborted.
DFence(t) ==
    /\ th[t].pc = "d_fence"
    /\ fence' = [fence EXCEPT ![t] = {u \in Threads \ {t} : InSeq(th[u].pc)}]
    /\ Set(t, [th[t] EXCEPT !.pc = "d_wait"])
    /\ UNCHANGED <<slab, spare, flag, link, lvl, depot, truth, gen, wipe, forked, lost>>

\* membarrier can fail. The drain then backs out: each marker still in place
\* goes back to the value it replaced, by compare-and-swap.
DFenceFail(t) ==
    /\ FenceCanFail
    /\ th[t].pc = "d_fence"
    /\ Set(t, [th[t] EXCEPT !.pc = IF "slab" \in th[t].tg THEN "d_b1" ELSE "d_b2"])
    /\ UNCHANGED <<slab, spare, flag, link, lvl, depot, truth, gen, wipe, forked, lost>>

Restore(w) == IF IsMark(w) THEN Zero ELSE w

DB1(t) ==
    /\ th[t].pc = "d_b1"
    /\ LET T == th[t] IN
        /\ slab' = [slab EXCEPT ![T.dc] = IF @ = Marker(T.g) THEN Restore(T.old) ELSE @]
        /\ Set(t, [T EXCEPT !.old = Zero, !.pc = IF "spare" \in T.tg THEN "d_b2" ELSE "d_rel"])
    /\ UNCHANGED <<spare, flag, link, lvl, depot, truth, gen, wipe, forked, lost>>

DB2(t) ==
    /\ th[t].pc = "d_b2"
    /\ LET T == th[t] IN
        /\ spare' = [spare EXCEPT ![T.dc] = IF @ = Marker(T.g) THEN Restore(T.osp) ELSE @]
        /\ Set(t, [T EXCEPT !.osp = Zero, !.pc = "d_rel"])
    /\ UNCHANGED <<slab, flag, link, lvl, depot, truth, gen, wipe, forked, lost>>

DWait(t) ==
    /\ th[t].pc = "d_wait"
    /\ fence[t] = {}
    /\ Set(t, [th[t] EXCEPT !.pc = IF "slab" \in th[t].tg THEN "d_r1" ELSE "d_r2"])
    /\ UNCHANGED <<slab, spare, flag, link, lvl, depot, truth, gen, wipe, forked, lost>>

\* Step 4: read each word again. Reading and storing zero are two steps.
DR1(t) ==
    /\ th[t].pc = "d_r1"
    /\ LET T == th[t] IN
        IF slab[T.dc] = Marker(T.g)
        THEN IF ZeroOnlyIfMag /\ ~IsMag(T.old)
             THEN Set(t, [T EXCEPT !.old = Zero, !.pc = AfterR1(T)])
             ELSE Set(t, [T EXCEPT !.w1 = IF IsMag(T.old) THEN T.old ELSE Zero,
                                   !.old = Zero, !.pc = "d_z1"])
        ELSE Set(t, [T EXCEPT !.old = Zero, !.pc = AfterR1(T)])
    /\ UNCHANGED <<slab, spare, flag, link, lvl, depot, truth, gen, wipe, forked, lost>>

DZ1(t) ==
    /\ th[t].pc = "d_z1"
    /\ slab' = [slab EXCEPT ![th[t].dc] = Zero]
    /\ Set(t, [th[t] EXCEPT !.pc = AfterR1(th[t])])
    /\ UNCHANGED <<spare, flag, link, lvl, depot, truth, gen, wipe, forked, lost>>

DR2(t) ==
    /\ th[t].pc = "d_r2"
    /\ LET T == th[t] IN
        IF spare[T.dc] = Marker(T.g)
        THEN IF ZeroOnlyIfMag /\ ~IsMag(T.osp)
             THEN Set(t, [T EXCEPT !.osp = Zero, !.pc = "d_send"])
             ELSE Set(t, [T EXCEPT !.w2 = IF IsMag(T.osp) THEN T.osp ELSE Zero,
                                   !.osp = Zero, !.pc = "d_z2"])
        ELSE Set(t, [T EXCEPT !.osp = Zero, !.pc = "d_send"])
    /\ UNCHANGED <<slab, spare, flag, link, lvl, depot, truth, gen, wipe, forked, lost>>

DZ2(t) ==
    /\ th[t].pc = "d_z2"
    /\ spare' = [spare EXCEPT ![th[t].dc] = Zero]
    /\ Set(t, [th[t] EXCEPT !.pc = "d_send"])
    /\ UNCHANGED <<slab, flag, link, lvl, depot, truth, gen, wipe, forked, lost>>

\* Send what the drainer now owns to the depot.
DSend(t) ==
    /\ th[t].pc = "d_send"
    /\ LET T == th[t] IN
        /\ depot' = depot \cup (IF IsMag(T.w1) THEN {T.w1.m} ELSE {}) \cup (Chain(T.w2, {}) \ {0})
        /\ lvl' = IF IsMag(T.w1) THEN [lvl EXCEPT ![T.w1.m] = T.w1.s] ELSE lvl
        /\ Set(t, [T EXCEPT !.w1 = Zero, !.w2 = Zero, !.pc = "d_rel"])
    /\ UNCHANGED <<slab, spare, flag, link, truth, gen, wipe, forked, lost>>

\* Step 5: release the claim. With generations the release is conditional,
\* because a forked child may have taken a stale claim over.
DRel(t) ==
    /\ th[t].pc = "d_rel"
    /\ LET T == th[t] IN
        flag' = IF Protocol = "v1" THEN flag
                ELSE IF Protocol = "gen" /\ flag[T.dc] # T.g THEN flag
                ELSE [flag EXCEPT ![T.dc] = 0]
    /\ Set(t, Fresh)
    /\ UNCHANGED <<slab, spare, link, lvl, depot, truth, gen, wipe, forked, lost>>

----------------------------------------------------------------------------
(* Fork, and the sweep that repairs a child ("claim" only) *)

\* The first slow path that sees the wiped page claims the repair with one
\* compare-and-swap and sweeps every slab.
RepairBegin(t) ==
    /\ wipe = 0
    /\ th[t].pc \in {"idle", "get"}
    /\ wipe' = 1
    /\ Set(t, [th[t] EXCEPT !.ret = th[t].pc, !.dc = 1, !.pc = "r_slab"])
    /\ UNCHANGED <<slab, spare, flag, link, lvl, depot, truth, gen, forked, lost>>

RSlab(t) ==
    /\ th[t].pc = "r_slab"
    /\ slab' = [slab EXCEPT ![th[t].dc] = IF IsMark(@) THEN Zero ELSE @]
    /\ Set(t, [th[t] EXCEPT !.pc = "r_spare"])
    /\ UNCHANGED <<spare, flag, link, lvl, depot, truth, gen, wipe, forked, lost>>

RSpare(t) ==
    /\ th[t].pc = "r_spare"
    /\ spare' = [spare EXCEPT ![th[t].dc] = IF IsMark(@) THEN Zero ELSE @]
    /\ Set(t, [th[t] EXCEPT !.pc = "r_flag"])
    /\ UNCHANGED <<slab, flag, link, lvl, depot, truth, gen, wipe, forked, lost>>

RFlag(t) ==
    /\ th[t].pc = "r_flag"
    /\ flag' = [flag EXCEPT ![th[t].dc] = 0]
    /\ IF th[t].dc < NCids
       THEN /\ Set(t, [th[t] EXCEPT !.dc = @ + 1, !.pc = "r_slab"])
            /\ UNCHANGED wipe
       ELSE /\ Set(t, [th[t] EXCEPT !.dc = 0, !.pc = th[t].ret, !.ret = "idle"])
            /\ wipe' = 2
    /\ UNCHANGED <<slab, spare, link, lvl, depot, truth, gen, forked, lost>>

\* Places a magazine is owned from, given the thread state.
Owners(m, thr, sl, sp, dp) ==
       {<<"slab", c>> : c \in {c \in Cids : sl[c].m = m}}
  \cup {<<"spare", c>> : c \in {c \in Cids : m \in Chain(sp[c], {})}}
  \cup (IF m \in dp THEN {<<"depot">>} ELSE {})
  \cup {<<"held", t>> : t \in {t \in Threads : thr[t].hw.m = m}}
  \cup {<<"drained", t>> : t \in {t \in Threads : thr[t].w1.m = m \/ m \in Chain(thr[t].w2, {})}}

\* Exchanged out of a slab by a drainer that has not yet re-read the word.
Pending(m, thr) ==
    \E t \in Threads : thr[t].old.m = m \/ m \in Chain(thr[t].osp, {})

Fork(t) ==
    /\ ForkMode # "none"
    /\ ~forked
    /\ Running(t)
    /\ ForkMode = "idle" => th[t].pc = "idle"
    /\ forked' = TRUE
    /\ gen' = gen + 1
    /\ wipe' = IF Protocol = "claim" THEN 0 ELSE wipe
    /\ th' = [u \in Threads |-> IF u = t THEN Restarted(th[t]) ELSE Fresh]
    /\ fence' = [u \in Threads |-> {}]
    /\ lost' = {m \in Mags : Owners(m, th', slab, spare, depot) = {} /\ ~Pending(m, th')}
    /\ UNCHANGED <<slab, spare, flag, link, lvl, depot, truth>>

----------------------------------------------------------------------------
(* The OS *)

\* Preemption, migration, a signal, or membarrier's interrupt: the sequence
\* restarts.
Abort(t) ==
    /\ InSeq(th[t].pc)
    /\ Set(t, Restarted(th[t]))
    /\ UNCHANGED <<slab, spare, flag, link, lvl, depot, truth, gen, wipe, forked, lost>>

Step(t) ==
    /\ Running(t)
    /\ \/ Start(t) \/ FastLoad(t) \/ FastCommit(t) \/ Get(t)
       \/ SpLoad(t) \/ SpLink(t) \/ SpCommit(t)
       \/ InsLoad(t) \/ InsCommit(t) \/ Put(t)
       \/ SppLoad(t) \/ SppStore(t) \/ SppCommit(t) \/ ToDepot(t)
       \/ DStart(t) \/ DX1(t) \/ DX2(t) \/ DFenceFail(t) \/ DB1(t) \/ DB2(t) \/ DWait(t)
       \/ DR1(t) \/ DZ1(t) \/ DR2(t) \/ DZ2(t) \/ DSend(t) \/ DRel(t)
       \/ RepairBegin(t) \/ RSlab(t) \/ RSpare(t) \/ RFlag(t)

\* A thread leaves a membarrier's wait set when its sequence ends.
Next ==
    \E t \in Threads :
        \/ (Running(t) /\ DFence(t))
        \/ Fork(t)
        \/ /\ Step(t) \/ Abort(t)
           /\ fence' = [d \in Threads |-> {u \in fence[d] : InSeq(th'[u].pc)}]

Spec == Init /\ [][Next]_vars

Symm == Permutations(Mutators) \cup Permutations(Drainers)

----------------------------------------------------------------------------

O(m) == Owners(m, th, slab, spare, depot)

\* No magazine has two owners, and every owner knows its true fill level.
NoDouble ==
    /\ \A m \in Mags : Cardinality(O(m)) <= 1
    /\ \A c \in Cids : slab[c] # Corrupt
    /\ \A c \in Cids : 0 \notin Chain(spare[c], {})
    /\ \A c \in Cids : IsMag(slab[c]) => slab[c].s = truth[slab[c].m]
    /\ \A t \in Threads :
        /\ IsMag(th[t].hw) => th[t].hw.s = truth[th[t].hw.m]
        /\ IsMag(th[t].w1) => th[t].w1.s = truth[th[t].w1.m]
        /\ \A m \in Chain(th[t].w2, {}) \ {0} : lvl[m] = truth[m]
    /\ \A m \in depot : lvl[m] = truth[m]
    /\ \A c \in Cids : \A m \in Chain(spare[c], {}) \ {0} : lvl[m] = truth[m]

\* No magazine is lost, apart from those a fork took with the threads it ended.
NoLoss == \A m \in Mags : O(m) # {} \/ Pending(m, th) \/ m \in lost

\* A marker or a claim belongs to a drain (or sweep) that is still running.
Quiet == \A t \in Threads : th[t].pc = "idle"
Stale(k) == Protocol = "gen" /\ k < gen
Clean ==
    (Quiet /\ wipe = 2) =>
        \A c \in Cids :
            /\ IsMark(slab[c]) => Stale(slab[c].k)
            /\ IsMark(spare[c]) => Stale(spare[c].k)
            /\ flag[c] # 0 => Stale(flag[c])

\* A compact view of a state for reading traces: per thread, the fields that
\* differ from a fresh thread.
Show(w) == IF IsMark(w) THEN <<"marker", w.k>>
           ELSE IF IsMag(w) THEN <<"mag", w.m, w.s>> ELSE "zero"
Alias == [
    slab |-> [c \in Cids |-> Show(slab[c])],
    spare |-> [c \in Cids |-> Show(spare[c])], flag |-> flag, depot |-> depot,
    truth |-> truth, lvl |-> lvl, link |-> [m \in Mags |-> link[m].m],
    fence |-> fence, gen |-> gen, wipe |-> wipe, lost |-> lost,
    th |-> [t \in Threads |->
        [f \in {f \in DOMAIN Fresh : th[t][f] # Fresh[f]} |->
            IF f \in {"rw", "rn", "hw", "old", "osp", "w1", "w2"}
            THEN Show(th[t][f]) ELSE th[t][f]]]]

=============================================================================
