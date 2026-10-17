------------------------------- MODULE Purge -------------------------------
(* Purge as a claim, racing with allocation and laundering.

   A purge claims the free bits of a range as an allocation would, returns
   the pages with madvise, and releases the bits. For large classes a second
   bitmap records which free objects are known to be zero.

   Objects and pages need not line up: Cover[o] is the set of pages object o
   touches. The design says "the bits of a fully free range" and leaves three
   things open, each a constant here:

     ClaimRule  which objects a purge of pages P claims:
                "overlap" every object that touches P; "inside" only those
                wholly inside P.
     ZeroRule   which claimed objects get their zero bit set:
                "inside" those wholly inside P; "claimed" all of them.
     ZeroOrder  whether zero bits are set "before" or "after" the free bits
                are released.

   The readings that work are overlap, inside, before. The rule behind all
   three: a zero bit is written only by whoever holds the object's claim, and
   only memory that is entirely the claimant's is discarded.

   A claim of several bits in one word is one compare-and-swap, modelled as
   one step that happens only if the bits are free. A failed compare-and-swap
   changes nothing, so leaving it out loses no behaviour. Summary bits are in
   Summary.tla. *)
EXTENDS Naturals, FiniteSets, Sequences, TLC

CONSTANTS
    Threads,
    Layout,     \* how objects sit on pages; see Cover below
    W,          \* objects per bitmap word
    ClaimRule, ZeroRule, ZeroOrder

\* Cover[o] is the set of pages object o touches.
Cover ==
    CASE Layout = "aligned"  -> <<{1}, {1}, {2}, {2}>>   \* two objects per page
      [] Layout = "straddle" -> <<{1}, {1, 2}, {2}>>     \* class size does not divide the page size
      [] Layout = "big"      -> <<{1, 2}, {3, 4}>>       \* two pages per object

Objs == 1..Len(Cover)
Pages == UNION {Cover[o] : o \in Objs}
WordOf(o) == (o - 1) \div W
WordIds == {WordOf(o) : o \in Objs}

VARIABLES
    free,    \* free[o]: the object's bit in the free bitmap
    zbit,    \* zbit[o]: the object's bit in the zero bitmap
    dirty,   \* dirty[o]: pages of o that hold data
    live,    \* objects the program holds
    th

vars == <<free, zbit, dirty, live, th>>

Idle == [pc |-> "idle", c |-> {}, P |-> {}, zeroed |-> FALSE, todo |-> {}]

Init ==
    /\ free = [o \in Objs |-> TRUE]
    /\ zbit = [o \in Objs |-> FALSE]
    /\ dirty = [o \in Objs |-> Cover[o]]
    /\ live = {}
    /\ th = [t \in Threads |-> Idle]

Set(t, T) == th' = [th EXCEPT ![t] = T]

----------------------------------------------------------------------------
(* The program: allocate (zeroed or not), write, free *)

\* Claim one free object.
Alloc(t) ==
    /\ th[t].pc = "idle"
    /\ \E o \in Objs, z \in BOOLEAN :
        /\ free[o]
        /\ free' = [free EXCEPT ![o] = FALSE]
        /\ Set(t, [Idle EXCEPT !.pc = "a_zero", !.c = {o}, !.zeroed = z])
    /\ UNCHANGED <<zbit, dirty, live>>

\* Test and clear the zero bit; a zeroed allocation clears the memory itself
\* unless the bit was set.
AllocZero(t) ==
    /\ th[t].pc = "a_zero"
    /\ LET o == CHOOSE o \in th[t].c : TRUE IN
        /\ zbit' = [zbit EXCEPT ![o] = FALSE]
        /\ dirty' = IF th[t].zeroed /\ ~zbit[o] THEN [dirty EXCEPT ![o] = {}] ELSE dirty
    /\ Set(t, [th[t] EXCEPT !.pc = "a_ret"])
    /\ UNCHANGED <<free, live>>

AllocReturn(t) ==
    /\ th[t].pc = "a_ret"
    /\ live' = live \cup th[t].c
    /\ Set(t, Idle)
    /\ UNCHANGED <<free, zbit, dirty>>

WriteObj(t) ==
    /\ th[t].pc = "idle"
    /\ \E o \in live :
        /\ dirty[o] # Cover[o]
        /\ dirty' = [dirty EXCEPT ![o] = Cover[o]]
    /\ UNCHANGED <<free, zbit, live, th>>

FreeBegin(t) ==
    /\ th[t].pc = "idle"
    /\ \E o \in live :
        /\ live' = live \ {o}
        /\ Set(t, [Idle EXCEPT !.pc = "f_or", !.c = {o}])
    /\ UNCHANGED <<free, zbit, dirty>>

\* A launder: one atomic OR.
FreeOr(t) ==
    /\ th[t].pc = "f_or"
    /\ free' = [o \in Objs |-> free[o] \/ o \in th[t].c]
    /\ Set(t, Idle)
    /\ UNCHANGED <<zbit, dirty, live>>

----------------------------------------------------------------------------
(* Purge *)

Inside(P) == {o \in Objs : Cover[o] \subseteq P}
Touching(P) == {o \in Objs : Cover[o] \cap P # {}}
Need(P) == IF ClaimRule = "overlap" THEN Touching(P) ELSE Inside(P)

PurgeBegin(t) ==
    /\ th[t].pc = "idle"
    /\ \E P \in SUBSET Pages \ {{}} :
        /\ Need(P) # {}
        /\ Set(t, [Idle EXCEPT !.pc = "p_claim", !.P = P, !.todo = Need(P)])
    /\ UNCHANGED <<free, zbit, dirty, live>>

\* Claim the range one bitmap word at a time, lowest first. If a word's bits
\* are not all free, give up and release what was claimed.
PurgeClaim(t) ==
    /\ th[t].pc = "p_claim"
    /\ LET T == th[t]
           w == CHOOSE w \in {WordOf(o) : o \in T.todo} :
                    \A v \in {WordOf(o) : o \in T.todo} : w <= v
           part == {o \in T.todo : WordOf(o) = w}
           rest == T.todo \ part IN
        IF \A o \in part : free[o]
        THEN /\ free' = [o \in Objs |-> free[o] /\ o \notin part]
             /\ Set(t, [T EXCEPT !.c = T.c \cup part, !.todo = rest,
                                 !.pc = IF rest = {} THEN "p_madv" ELSE "p_claim"])
        ELSE /\ UNCHANGED free
             /\ Set(t, [T EXCEPT !.todo = {}, !.P = {},
                                 !.pc = IF T.c = {} THEN "idle" ELSE "p_rel"])
    /\ UNCHANGED <<zbit, dirty, live>>

\* madvise(MADV_DONTNEED): the pages read as zero afterwards.
PurgeMadvise(t) ==
    /\ th[t].pc = "p_madv"
    /\ dirty' = [o \in Objs |-> dirty[o] \ th[t].P]
    /\ Set(t, [th[t] EXCEPT !.pc = IF ZeroOrder = "before" THEN "p_zero" ELSE "p_rel"])
    /\ UNCHANGED <<free, zbit, live>>

PurgeZero(t) ==
    /\ th[t].pc = "p_zero"
    /\ LET T == th[t]
           z == IF ZeroRule = "inside" THEN Inside(T.P) ELSE Touching(T.P) IN
        /\ zbit' = [o \in Objs |-> zbit[o] \/ o \in z]
        /\ Set(t, IF ZeroOrder = "before" THEN [T EXCEPT !.pc = "p_rel"] ELSE Idle)
    /\ UNCHANGED <<free, dirty, live>>

\* Release the claimed bits, one atomic OR per word.
PurgeRelease(t) ==
    /\ th[t].pc = "p_rel"
    /\ LET T == th[t]
           w == CHOOSE w \in {WordOf(o) : o \in T.c} : TRUE
           part == {o \in T.c : WordOf(o) = w}
           rest == T.c \ part IN
        /\ free' = [o \in Objs |-> free[o] \/ o \in part]
        /\ Set(t, IF rest # {} THEN [T EXCEPT !.c = rest]
                  ELSE IF ZeroOrder = "after" /\ T.P # {} THEN [T EXCEPT !.c = {}, !.pc = "p_zero"]
                  ELSE Idle)
    /\ UNCHANGED <<zbit, dirty, live>>

Next == \E t \in Threads :
    \/ Alloc(t) \/ AllocZero(t) \/ AllocReturn(t) \/ WriteObj(t)
    \/ FreeBegin(t) \/ FreeOr(t)
    \/ PurgeBegin(t) \/ PurgeClaim(t) \/ PurgeMadvise(t) \/ PurgeZero(t) \/ PurgeRelease(t)

Spec == Init /\ [][Next]_vars

Symm == Permutations(Threads)

----------------------------------------------------------------------------

Held(t) == th[t].c

\* Each object is free, live, or claimed by exactly one thread.
Partition ==
    \A o \in Objs :
        Cardinality({t \in Threads : o \in Held(t)})
            + (IF free[o] THEN 1 ELSE 0) + (IF o \in live THEN 1 ELSE 0) = 1

\* A purge discards only memory of objects it has claimed. So no live
\* object's memory, and none in a magazine, is ever purged.
PurgeSafe ==
    \A t \in Threads :
        th[t].pc = "p_madv" => Touching(th[t].P) \subseteq th[t].c

\* A set zero bit is true, and no live object has one.
ZeroBitSound ==
    \A o \in Objs : zbit[o] => dirty[o] = {} /\ o \notin live

\* A zeroed allocation returns zero memory.
ZeroedIsZero ==
    \A t \in Threads :
        th[t].pc = "a_ret" /\ th[t].zeroed => \A o \in th[t].c : dirty[o] = {}

\* A launder or release never finds its bit already set, which the allocator
\* would report as a double free.
NoFalseDoubleFree ==
    \A t \in Threads :
        th[t].pc \in {"f_or", "p_rel"} => \A o \in th[t].c : ~free[o]

=============================================================================
