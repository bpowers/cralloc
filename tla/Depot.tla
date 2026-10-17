------------------------------- MODULE Depot -------------------------------
(* A depot stack: a lock-free stack of magazines.

   The head is one word holding a magazine number and a version counter, and
   changes only by compare-and-swap. A magazine's link lives in its own slot 0,
   which a thread may read after the magazine has left the stack (a stale
   read), and which the magazine's owner overwrites freely (the per-cid spare
   stacks use the same slot).

   UseVersion = FALSE drops the counter and shows the ABA failure.
   VerMod = 0 is an unbounded counter, explored up to MaxVer.
   VerMod = k > 0 wraps the counter modulo k, standing in for 2^32. *)
EXTENDS Naturals, FiniteSets, TLC

CONSTANTS Threads, NMags, UseVersion, VerMod, MaxVer

Mags == 1..NMags
None == 0

VARIABLES
    head,   \* [top |-> magazine or None, ver |-> counter]
    link,   \* link[m]: slot 0 of magazine m
    pc,     \* per thread
    seen,   \* per thread: the head it read
    next,   \* per thread: the link it read (pop)
    mag,    \* per thread: the magazine it is pushing
    own     \* per thread: magazines it holds privately

vars == <<head, link, pc, seen, next, mag, own>>

Bump(v) == IF ~UseVersion THEN 0
           ELSE IF VerMod = 0 THEN v + 1
           ELSE (v + 1) % VerMod

\* Registers are reset when an operation ends, to keep the state space small.
NoHead == [top |-> None, ver |-> 0]

Init ==
    /\ head = [top |-> 1, ver |-> 0]
    /\ link = [m \in Mags |-> IF m < NMags THEN m + 1 ELSE None]
    /\ pc = [t \in Threads |-> "idle"]
    /\ seen = [t \in Threads |-> NoHead]
    /\ next = [t \in Threads |-> None]
    /\ mag = [t \in Threads |-> None]
    /\ own = [t \in Threads |-> {}]

PopRead(t) ==
    /\ pc[t] = "idle"
    /\ head.top # None
    /\ seen' = [seen EXCEPT ![t] = head]
    /\ pc' = [pc EXCEPT ![t] = "pop_link"]
    /\ UNCHANGED <<head, link, next, mag, own>>

\* The magazine may have been popped by someone else by now; its memory is
\* never unmapped, so the read is safe and returns whatever is there.
PopLink(t) ==
    /\ pc[t] = "pop_link"
    /\ next' = [next EXCEPT ![t] = link[seen[t].top]]
    /\ pc' = [pc EXCEPT ![t] = "pop_cas"]
    /\ UNCHANGED <<head, link, seen, mag, own>>

PopCas(t) ==
    /\ pc[t] = "pop_cas"
    /\ IF head = seen[t]
       THEN /\ head' = [top |-> next[t], ver |-> Bump(head.ver)]
            /\ own' = [own EXCEPT ![t] = @ \cup {seen[t].top}]
       ELSE UNCHANGED <<head, own>>
    /\ pc' = [pc EXCEPT ![t] = "idle"]
    /\ seen' = [seen EXCEPT ![t] = NoHead]
    /\ next' = [next EXCEPT ![t] = None]
    /\ UNCHANGED <<link, mag>>

PushRead(t) ==
    /\ \/ /\ pc[t] = "idle"
          /\ \E m \in own[t] : mag' = [mag EXCEPT ![t] = m]
       \/ /\ pc[t] = "push_read"
          /\ UNCHANGED mag
    /\ seen' = [seen EXCEPT ![t] = head]
    /\ pc' = [pc EXCEPT ![t] = "push_link"]
    /\ UNCHANGED <<head, link, next, own>>

PushLink(t) ==
    /\ pc[t] = "push_link"
    /\ link' = [link EXCEPT ![mag[t]] = seen[t].top]
    /\ pc' = [pc EXCEPT ![t] = "push_cas"]
    /\ UNCHANGED <<head, seen, next, mag, own>>

PushCas(t) ==
    /\ pc[t] = "push_cas"
    /\ IF head = seen[t]
       THEN /\ head' = [top |-> mag[t], ver |-> Bump(head.ver)]
            /\ own' = [own EXCEPT ![t] = @ \ {mag[t]}]
            /\ pc' = [pc EXCEPT ![t] = "idle"]
            /\ mag' = [mag EXCEPT ![t] = None]
       ELSE /\ pc' = [pc EXCEPT ![t] = "push_read"]
            /\ UNCHANGED <<head, own, mag>>
    /\ seen' = [seen EXCEPT ![t] = NoHead]
    /\ UNCHANGED <<link, next>>

\* The owner of a magazine uses its slot 0 for other things. One other value
\* is enough: what matters is that the link no longer says what a stale
\* reader saw.
Scribble(t) ==
    /\ pc[t] = "idle"
    /\ \E m \in own[t] :
        /\ link[m] # None
        /\ link' = [link EXCEPT ![m] = None]
    /\ UNCHANGED <<head, pc, seen, next, mag, own>>

Next == \E t \in Threads :
    \/ PopRead(t) \/ PopLink(t) \/ PopCas(t)
    \/ PushRead(t) \/ PushLink(t) \/ PushCas(t)
    \/ Scribble(t)

Spec == Init /\ [][Next]_vars

Symm == Permutations(Threads)

----------------------------------------------------------------------------

\* The magazines reachable from the head, or {None} if the chain loops.
RECURSIVE Walk(_, _)
Walk(m, acc) ==
    IF m = None THEN acc
    ELSE IF m \in acc THEN {None}
    ELSE Walk(link[m], acc \cup {m})

InStack == Walk(head.top, {})

Held == UNION {own[t] : t \in Threads}

TypeOK ==
    /\ head.top \in Mags \cup {None}
    /\ \A m \in Mags : link[m] \in Mags \cup {None}
    /\ \A t \in Threads : own[t] \subseteq Mags

\* Every magazine is in the stack or held by exactly one thread, never both.
OneOwner ==
    /\ None \notin InStack
    /\ InStack \cap Held = {}
    /\ InStack \cup Held = Mags
    /\ \A t, u \in Threads : t # u => own[t] \cap own[u] = {}

VerBound == VerMod # 0 \/ head.ver <= MaxVer

=============================================================================
