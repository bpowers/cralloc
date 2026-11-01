-------------------------------- MODULE Tail --------------------------------
(* The boundary of a region's huge-page tail (crates/cralloc/src/back/tail.rs).

   A region's huge-page part has units 1..N. The kernel holds advice per unit,
   "on" or "off". The tail word says where the advice is off: `from` = 0 for
   nowhere, else every unit from `from` up. Beside it in the same word: `low`,
   the lowest unit the purge returned part of since the boundary last came
   down (0 for none); `want`, a claim's mark that the advice should go back
   on; and the owner bit, `busy`, with the counted interval it was taken in,
   and `repair`, which says the kernel's advice may differ from the word.

   Every write of the word is one compare-and-swap, modelled as one step. An
   advice change is: take the owner bit (one CAS from a word whose bit is clear
   or stale), make the call (one step, which the OS may refuse after acting on
   any part of its range), give the bit back (one CAS that keeps marks and
   notes set meanwhile, or, if the bit was taken over, sets `repair`). An owner
   that finds the bit taken skips. An owner whose bit is two intervals old is
   taken over, and the new owner reconciles the kernel's advice with the word
   first.

   The OS may stop a thread for good at any step (a fork leaves it behind) and
   may let intervals pass while a thread is anywhere, so an owner can be taken
   over while still alive and make its call late.

   Properties:
     Agree      whenever no live thread owns the region or is about to write
                advice, and `repair` is clear, the kernel's advice is the word's.
     OffOnlyReturned  advice is off only at or above a unit the purge returned
                part of: it never comes off where nothing went back.
   No thread waits: a thread that finds the bit owned does not take a step on
   it (there is no waiting state), and nothing loops: each change is a fixed
   sequence of steps.

   Constants:
     Units      N, the units of the huge-page part.
     Repair     TRUE: the design. FALSE: a taken-over owner's late call is not
                followed by a repair, and Agree fails (Tail_norepair).
     ForkRepair TRUE: the design. The child of a fork, at its first slow path
                (where it draws its generation), asks for a repair on every
                tail word it inherited, so that it never trusts advice a thread
                the fork cut off may have changed mid-change. FALSE: an owner
                taken over while still alive (stopped for two intervals in the
                middle of its change) makes its late call, a fork cuts it off
                before it can ask for the repair, and in the child the region's
                advice differs from the word until it next changes (Tail_orphan).
   `orphan` records that such a thread existed; it is for reading traces.

   Marks (Back::raise_marked). A claim that sets `want` also sets the heap's
   flag `marks`. A looker clears the flag (one CAS) and then looks at the word:
   if the region carries a mark and no live owner holds the bit, it takes the
   bit to raise; if an owner holds it, it leaves the mark and does NOT set the
   flag again, or an owner that does not run would keep the background thread
   awake for ever. The owner answers for the mark instead: a give-back that
   leaves `want` in the word sets the flag, and so does a give-back that finds
   its bit taken over. A refused raise keeps its mark. The fork child's repair
   clears the owner bit (no thread of the child owns it, but for the forking
   thread itself, whose give-back then behaves as a taken-over owner's) and
   sets the flag if the word carries a mark.

     MarkVisible  whenever every live thread is idle and no fork repair is
                pending, a word carrying a mark has the flag set. With a looker
                that runs while the flag is set (the background thread, which
                does not sleep while marks are due, or the next heartbeat), this
                is the liveness the code relies on: every mark is looked at
                again until a raise takes it. Answered checks it as a temporal
                property under weak fairness of the live threads' steps
                (Tail_live): a mark not held by an owner is eventually taken by
                a raise, or the word loses it. *)
EXTENDS Naturals, FiniteSets, TLC

CONSTANTS Threads, Units, Repair, MaxEpoch, ForkRepair, ChildClears, Lowerings, RetryTake

U == 1..Units
StaleAfter == 2

Want(from) == [u \in U |-> IF from # 0 /\ u >= from THEN "off" ELSE "on"]

VARIABLES
    adv,      \* the kernel's advice, per unit
    w,        \* the tail word: [from, low, want, busy, repair, stamp]
    epoch,    \* the counted interval
    returned, \* units the purge has returned part of
    th,       \* each thread: pc, the word it took (mine), what its call does
    alive,
    orphan,   \* a fork cut off an owner that had been taken over and had made
              \* its late call, before it could ask for the repair
    child,    \* a fork happened and the child has not yet run its repair
    marks,    \* the heap's flag: some region may carry a mark not yet looked at
    lowered   \* lowerings taken, counted only if Lowerings # 0

vars == <<adv, w, epoch, returned, th, alive, orphan, child, marks, lowered>>

Zero == [from |-> 0, low |-> 0, want |-> FALSE, busy |-> FALSE, repair |-> FALSE, stamp |-> 0]
Idle == [pc |-> "idle", mine |-> Zero, op |-> "none", lo |-> 0, hi |-> 0, ok |-> TRUE, next |-> Zero,
         called |-> FALSE,   \* it has made a call since it took the bit
         seen |-> Zero]      \* the word a looker loaded, which its take compares against

Init ==
    /\ adv = [u \in U |-> "on"]
    /\ w = Zero
    /\ epoch = 0
    /\ returned = {}
    /\ th = [t \in Threads |-> Idle]
    /\ alive = Threads
    /\ orphan = FALSE
    /\ child = FALSE
    /\ marks = FALSE
    /\ lowered = 0

Owned(x) == x.busy /\ epoch - x.stamp < StaleAfter

\* A walker returns part of unit u and notes it: CAS that lowers `low`.
Note(t, u) ==
    /\ t \in alive /\ th[t].pc = "idle"
    /\ returned' = returned \cup {u}
    /\ IF (w.low # 0 /\ w.low <= u) \/ (w.from # 0 /\ u >= w.from)
       THEN UNCHANGED w
       ELSE w' = [w EXCEPT !.low = u]
    /\ UNCHANGED <<adv, epoch, th, alive, orphan, child, marks, lowered>>

\* A claim reaches into unit u, above the boundary: sets the mark.
Mark(t, u) ==
    /\ t \in alive /\ th[t].pc = "idle"
    /\ w.from # 0 /\ u >= w.from /\ ~w.want
    /\ w' = [w EXCEPT !.want = TRUE]
    /\ marks' = TRUE
    /\ UNCHANGED <<adv, epoch, returned, th, alive, orphan, child, lowered>>

\* A looker clears the flag, then looks at the word (Look2).
Look(t) ==
    /\ t \in alive /\ th[t].pc = "idle"
    /\ marks
    /\ marks' = FALSE
    /\ th' = [th EXCEPT ![t].pc = "look"]
    /\ UNCHANGED <<adv, w, epoch, returned, alive, orphan, child, lowered>>

\* The looker loads the word (raise_moved's load), to take it with a CAS next.
LookLoad(t) ==
    /\ t \in alive /\ th[t].pc = "look"
    /\ th' = [th EXCEPT ![t].pc = "take", ![t].seen = w]
    /\ UNCHANGED <<adv, w, epoch, returned, alive, orphan, child, marks, lowered>>

\* The word it loaded has no mark, or an owner: it leaves the word and the flag alone.
LookSkip(t) ==
    /\ t \in alive /\ th[t].pc = "take"
    /\ (~th[t].seen.want \/ Owned(th[t].seen))
    /\ th' = [th EXCEPT ![t] = Idle]
    /\ UNCHANGED <<adv, w, epoch, returned, alive, orphan, child, marks, lowered>>

\* Its CAS fails: the word changed since the load (a note, a mark, another owner). With
\* RetryTake, the code's: it loads again. Without, it gives up, and a mark it leaves is seen
\* by nobody (Tail_lost).
TakeLost(t) ==
    /\ t \in alive /\ th[t].pc = "take"
    /\ th[t].seen.want /\ ~Owned(th[t].seen)
    /\ w # th[t].seen
    /\ th' = [th EXCEPT ![t] = IF RetryTake THEN [th[t] EXCEPT !.pc = "look"] ELSE Idle]
    /\ UNCHANGED <<adv, w, epoch, returned, alive, orphan, child, marks, lowered>>

\* Take the owner bit, for a lowering or a raise; the call's range is fixed
\* from the word as taken. A thread that finds the bit owned does nothing.
Take(t, kind) ==
    /\ t \in alive
    /\ th[t].pc = IF kind = "raise" THEN "take" ELSE "idle"
    \* A raise's CAS succeeds only against the word it loaded.
    /\ (kind = "raise" => w = th[t].seen)
    /\ ~Owned(w)
    /\ \/ kind = "lower" /\ (w.low # 0 \/ w.repair \/ w.busy)
          \* Lowerings # 0 bounds them, for the liveness check: in the code a region
          \* is lowered once a pass, and not again in an interval the kernel refused.
          /\ (Lowerings = 0 \/ lowered < Lowerings)
       \/ kind = "raise" /\ w.want
    /\ lowered' = IF kind = "lower" /\ Lowerings # 0 THEN lowered + 1 ELSE lowered
    \* A raise goes up to any unit above the boundary, or to the end.
    /\ \E to \in (w.from + 1)..(Units + 1) :
       \* The owner clears the repair bit as it takes it and remembers that it
       \* must reconcile, so that a repair asked for during its ownership (by an
       \* owner it took over) is one it can tell from the one it answers.
       LET mine == [w EXCEPT !.busy = TRUE, !.repair = FALSE, !.stamp = epoch]
           old == IF w.from = 0 THEN Units + 1 ELSE w.from
       IN /\ w' = mine
          /\ th' = [th EXCEPT ![t] = [Idle EXCEPT
                 !.pc = IF w.repair \/ w.busy THEN "reconcile" ELSE "call",
                 !.mine = mine,
                 !.op = kind,
                 \* lowering: off over [low, old); raising: on over [from, to)
                 !.lo = IF kind = "lower" THEN w.low ELSE w.from,
                 !.hi = IF kind = "lower" THEN old - 1 ELSE to - 1]]
    /\ UNCHANGED <<adv, epoch, returned, alive, orphan, child, marks>>

\* Reconcile: make the kernel agree with `from` as taken. The OS may refuse
\* after acting on any subset.
Reconcile(t) ==
    /\ t \in alive /\ th[t].pc = "reconcile"
    /\ \E done \in SUBSET U, ok \in BOOLEAN :
          /\ (ok => done = U)
          /\ adv' = [u \in U |-> IF u \in done THEN Want(th[t].mine.from)[u] ELSE adv[u]]
          /\ th' = [th EXCEPT ![t].pc = IF ok THEN "call" ELSE "give",
                             ![t].ok = ok, ![t].called = TRUE,
                             ![t].next = [th[t].mine EXCEPT !.busy = FALSE, !.repair = ~ok]]
    /\ UNCHANGED <<w, epoch, returned, alive, orphan, child, marks, lowered>>

\* The advice call. A lowering with nothing below the old boundary, or a raise
\* of an absent tail, makes none.
Call(t) ==
    /\ t \in alive /\ th[t].pc = "call"
    /\ LET m == th[t].mine
           lo == th[t].lo
           hi == th[t].hi
           real == lo # 0 /\ lo <= hi
           a == IF th[t].op = "lower" THEN "off" ELSE "on"
           base == [m EXCEPT !.busy = FALSE, !.repair = FALSE]
       IN IF ~real
          THEN /\ th' = [th EXCEPT ![t].pc = "give",
                    ![t].next = IF th[t].op = "lower" THEN [base EXCEPT !.low = 0]
                                ELSE [base EXCEPT !.want = FALSE]]
               /\ UNCHANGED adv
          ELSE \E done \in SUBSET (lo..hi), ok \in BOOLEAN :
               /\ (ok => done = lo..hi)
               /\ adv' = [u \in U |-> IF u \in done THEN a ELSE adv[u]]
               /\ th' = [th EXCEPT ![t].pc = "give", ![t].called = TRUE,
                    ![t].next =
                      IF ~ok THEN [base EXCEPT !.repair = TRUE,
                                               !.low = IF th[t].op = "lower" THEN m.low ELSE m.low,
                                               !.want = m.want]
                      ELSE IF th[t].op = "lower"
                      THEN [base EXCEPT !.from = lo, !.low = 0]
                      ELSE [base EXCEPT !.from = IF hi >= Units THEN 0 ELSE hi + 1,
                                        !.want = FALSE]]
    /\ UNCHANGED <<w, epoch, returned, alive, orphan, child, marks, lowered>>

\* Give the bit back: if the word still carries this thread's bit, write the
\* result, keeping a mark or a lower note set meanwhile; else ask for repair.
Give(t) ==
    /\ t \in alive /\ th[t].pc = "give"
    /\ LET m == th[t].mine
           n == th[t].next
       IN IF w.busy /\ w.stamp = m.stamp
          THEN LET low0 == IF w.low = m.low THEN n.low
                           ELSE IF n.low = 0 THEN w.low
                           ELSE IF w.low = 0 THEN n.low
                           ELSE IF n.low < w.low THEN n.low ELSE w.low
                   low1 == IF n.from # 0 /\ low0 >= n.from THEN 0 ELSE low0
                   want1 == n.want \/ (w.want /\ ~m.want)
               IN /\ w' = [n EXCEPT !.busy = FALSE, !.stamp = m.stamp,
                             !.want = want1,
                             !.repair = n.repair \/ (Repair /\ w.repair),
                             !.low = low1]
                  \* The owner answers for a mark it leaves.
                  /\ marks' = (marks \/ want1)
          ELSE /\ w' = [w EXCEPT !.repair = IF Repair THEN TRUE ELSE w.repair]
               /\ marks' = (marks \/ w.want)
    /\ th' = [th EXCEPT ![t] = Idle]
    /\ UNCHANGED <<adv, epoch, returned, alive, orphan, child, lowered>>

\* Time passes while the program runs.
Tick ==
    /\ epoch < MaxEpoch
    /\ epoch' = epoch + 1
    /\ UNCHANGED <<adv, w, returned, th, alive, orphan, child, marks, lowered>>

\* A fork leaves a thread behind wherever it is.
Vanish(t) ==
    /\ t \in alive /\ Cardinality(alive) > 1
    /\ alive' = alive \ {t}
    /\ orphan' = (orphan \/ (th[t].called
                       /\ ~(w.busy /\ w.stamp = th[t].mine.stamp)))
    /\ child' = ForkRepair
    /\ UNCHANGED <<adv, w, epoch, returned, th, marks, lowered>>

\* The child's first slow path asks for a repair of the tail word and, with
\* ChildClears, clears its owner bit (one CAS): no thread of the child owns it,
\* and the forking thread, if it was mid-change, gives back as a taken-over
\* owner. A word carrying a mark sets the flag, as its owner would have.
ChildRepair ==
    /\ child
    /\ w' = [w EXCEPT !.repair = TRUE, !.busy = IF ChildClears THEN FALSE ELSE w.busy]
    /\ marks' = (marks \/ (ChildClears /\ w.want))
    /\ child' = FALSE
    /\ UNCHANGED <<adv, epoch, returned, th, alive, orphan, lowered>>

Next ==
    \/ \E t \in Threads, u \in U : Note(t, u) \/ Mark(t, u)
    \/ \E t \in Threads : Take(t, "lower") \/ Take(t, "raise")
    \/ \E t \in Threads : Reconcile(t) \/ Call(t) \/ Give(t) \/ Vanish(t)
    \/ \E t \in Threads : Look(t) \/ LookLoad(t) \/ LookSkip(t) \/ TakeLost(t)
    \/ Tick
    \/ ChildRepair

Spec == Init /\ [][Next]_vars

\* The live threads' steps are taken when enabled, and a look whenever it is
\* enabled again and again: the background thread runs while marks are due,
\* and a heartbeat looks before it steps a pass, but a thread's own lowerings
\* leave it idle only now and then (strong fairness). Claims, notes, forks
\* and time are not forced.
Step(t) ==
    \/ LookLoad(t) \/ LookSkip(t) \/ TakeLost(t) \/ Take(t, "raise") \/ Reconcile(t) \/ Call(t)
    \/ Give(t)
LiveSpec ==
    /\ Spec
    /\ \A t \in Threads : SF_vars(t \in alive /\ Look(t)) /\ WF_vars(t \in alive /\ Step(t))
    /\ WF_vars(ChildRepair)

\* Nobody live is mid-change, nobody can still write advice late, the bit is
\* free, nothing asks for repair: the kernel agrees with the word.
Quiet ==
    /\ \A t \in alive : th[t].pc = "idle"
    /\ ~child
    /\ ~w.busy
    /\ ~w.repair
Agree == Quiet => adv = Want(w.from)

\* A mark nobody is handling is visible to the next looker.
MarkVisible ==
    (w.want /\ ~child /\ \A t \in alive : th[t].pc = "idle") => marks

\* Every mark is eventually taken by a raise (the owner bit taken for one), or
\* the word no longer carries it.
Answered == w.want ~> (~w.want \/ \E t \in alive : th[t].op = "raise" /\ th[t].pc # "idle")

\* Off only at or above a unit that was returned in part.
OffOnlyReturned == \A u \in U : adv[u] = "off" => \E r \in returned : r <= u

TypeOK ==
    /\ w.from \in 0..Units /\ w.low \in 0..Units
    /\ adv \in [U -> {"on", "off"}]
=============================================================================
