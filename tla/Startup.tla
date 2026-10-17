------------------------------ MODULE Startup ------------------------------
(* Start-up: racing initializers publish the heap with one compare-and-swap,
   the loser unmaps its reservation, and other threads are already calling in.

   The OS is the address-space budget. A reservation asks for Want units and
   gets what is left if that is less ("as much as the process is allowed"),
   or fails if nothing is left. With Budget >= Want * (number of threads) no
   reservation is ever short, which is the ordinary case: 16 TiB out of
   128 TiB of address space fits seven times. A smaller Budget stands for
   RLIMIT_AS, or for eight or more threads making their first call at once.

   Everything a thread needs to know about the heap (its base, its size,
   whether rseq is usable) is taken to be in the reservation before the
   compare-and-swap, so publishing it is one step.

   Crash = TRUE lets every thread but one vanish at an arbitrary step, once:
   a fork. *)
EXTENDS Naturals, FiniteSets, TLC

CONSTANTS Threads, Want, Budget, Crash

None == "none"

VARIABLES
    heap,     \* the published heap: None, or the thread whose reservation it is
    size,     \* size[t]: the size of t's reservation while it is mapped, else 0
    refs,     \* reservations that shared state (slabs, the depot) points into
    th,
    crashed,
    orphans   \* ghost: reservations whose owner vanished in the fork

vars == <<heap, size, refs, th, crashed, orphans>>

Idle == [pc |-> "idle", h |-> None, res |-> "none"]

Init ==
    /\ heap = None
    /\ size = [t \in Threads |-> 0]
    /\ refs = {}
    /\ th = [t \in Threads |-> Idle]
    /\ crashed = FALSE
    /\ orphans = {}

RECURSIVE Sum(_)
Sum(S) == IF S = {} THEN 0 ELSE LET x == CHOOSE x \in S : TRUE IN size[x] + Sum(S \ {x})
Avail == Budget - Sum(Threads)
Min(a, b) == IF a < b THEN a ELSE b

Set(t, T) == th' = [th EXCEPT ![t] = T]

\* Any call into the allocator's slow path.
Call(t) ==
    /\ th[t].pc = "idle"
    /\ Set(t, [Idle EXCEPT !.pc = "read"])
    /\ UNCHANGED <<heap, size, refs, crashed, orphans>>

Read(t) ==
    /\ th[t].pc = "read"
    /\ Set(t, [th[t] EXCEPT !.h = heap, !.pc = IF heap = None THEN "reserve" ELSE "use"])
    /\ UNCHANGED <<heap, size, refs, crashed, orphans>>

Reserve(t) ==
    /\ th[t].pc = "reserve"
    /\ IF Avail = 0
       THEN /\ Set(t, [Idle EXCEPT !.res = "null"])
            /\ UNCHANGED size
       ELSE /\ size' = [size EXCEPT ![t] = Min(Want, Avail)]
            /\ Set(t, [th[t] EXCEPT !.pc = "cas"])
    /\ UNCHANGED <<heap, refs, crashed, orphans>>

Publish(t) ==
    /\ th[t].pc = "cas"
    /\ IF heap = None
       THEN /\ heap' = t
            /\ Set(t, [th[t] EXCEPT !.h = t, !.pc = "use"])
       ELSE /\ UNCHANGED heap
            /\ Set(t, [th[t] EXCEPT !.h = heap, !.pc = "unmap"])
    /\ UNCHANGED <<size, refs, crashed, orphans>>

Unmap(t) ==
    /\ th[t].pc = "unmap"
    /\ size' = [size EXCEPT ![t] = 0]
    /\ Set(t, [th[t] EXCEPT !.pc = "use"])
    /\ UNCHANGED <<heap, refs, crashed, orphans>>

\* Serve the request from the heap the thread saw, leaving pointers into it
\* in shared state.
Use(t) ==
    /\ th[t].pc = "use"
    /\ refs' = refs \cup {th[t].h}
    /\ Set(t, [Idle EXCEPT !.res = "ok"])
    /\ UNCHANGED <<heap, size, crashed, orphans>>

CrashAllBut(t) ==
    /\ Crash /\ ~crashed
    /\ crashed' = TRUE
    /\ orphans' = {u \in Threads \ {t} : size[u] > 0 /\ heap # u}
    /\ th' = [u \in Threads |-> IF u = t THEN th[t] ELSE Idle]
    /\ UNCHANGED <<heap, size, refs>>

Next == \E t \in Threads :
    Call(t) \/ Read(t) \/ Reserve(t) \/ Publish(t) \/ Unmap(t) \/ Use(t) \/ CrashAllBut(t)

Spec == Init /\ [][Next]_vars

----------------------------------------------------------------------------

\* The heap, once published, stays mapped, and nothing ever points anywhere
\* else. No thread uses a reservation that was or will be unmapped.
OneHeap ==
    /\ heap # None => size[heap] > 0
    /\ refs \subseteq {heap}
    /\ \A t \in Threads : th[t].pc = "use" => th[t].h = heap /\ heap # None

\* Only the heap and a reservation its owner is still working on are mapped.
\* After a fork, the reservations of vanished threads stay mapped for good:
\* address space, not memory.
NoLeak ==
    \A t \in Threads :
        size[t] > 0 => \/ heap = t
                       \/ th[t].pc \in {"cas", "unmap"}
                       \/ (crashed /\ t \in orphans)

\* The two below hold only when the budget covers every racer.

\* The published heap is as large as the budget allows.
FullSize == heap # None => size[heap] = Min(Want, Budget)

\* A request fails only if there is no address space at all.
NoSpuriousNull == \A t \in Threads : th[t].res = "null" => Budget = 0 \/ crashed

=============================================================================
