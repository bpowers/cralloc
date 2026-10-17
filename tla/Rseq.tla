-------------------------------- MODULE Rseq --------------------------------
(* Restartable sequences and concurrency ids.

   Threads run on cids. The OS may stop a thread between any two steps and
   later resume it on any cid no running thread holds. A thread stopped inside
   a sequence restarts the sequence from the top.

   Pop, push and the conditional install are each several steps that commit
   with the last one. Everything outside the three routines (where a spare
   magazine comes from, where a retired one goes) is one atomic step on a pool
   here; Exchange.tla and Depot.tla model those parts.

   BadCid = TRUE lets the OS hand a thread a cid that is out of range, which
   any number of threads may hold at once. A routine compares the cid with
   the slab count first and, if it is out of range, leaves for the slow path,
   which serves the call from the back end and touches no slab.

   Restart = FALSE lets a stopped thread carry on where it was, and
   Exclusive = FALSE lets two running threads share a cid. Each breaks the
   invariant, which shows the model depends on exactly the two OS guarantees
   the design trusts. *)
EXTENDS Naturals, FiniteSets, TLC

CONSTANTS
    Threads,
    NCids,
    NMags,
    NObjs,
    Slots,      \* slots per magazine: slot 0 is the link, the last is unused
    Restart,
    Exclusive,
    BadCid

Cids == 1..NCids
Mags == 1..NMags
Objs == 1..NObjs
Cap  == Slots - 2

\* A slab word is the address of slot s of magazine m. Zero is the all-zero word.
Zero == [m |-> 0, s |-> 0]
Word(m, s) == [m |-> m, s |-> s]
IsEmpty(w) == w.s <= 1               \* word & mask = 0: no magazine, or empty
IsFull(w)  == w.s = 0 \/ w.s = Slots - 1   \* (word + 8) & mask = 0

VARIABLES
    cid,    \* cid[t]: the cid t runs on, or 0 when it is not running
    slab,   \* slab[c]: the slab word of cid c (one class)
    slot,   \* slot[m][i]: the memory of magazine m's object slots
    pool,   \* unloaded magazines nobody holds, as words
    back,   \* objects free in the back end
    user,   \* user[t]: objects the program holds through thread t
    pc,
    rc,     \* register: the cid the sequence read
    rw,     \* register: the slab word the sequence read
    ro,     \* register: the object pop read
    obj,    \* the object being freed, or 0
    hw      \* the magazine the slow path holds privately, as a word

vars == <<cid, slab, slot, pool, back, user, pc, rc, rw, ro, obj, hw>>

Init ==
    /\ cid = [t \in Threads |-> 0]
    /\ slab = [c \in Cids |-> Zero]
    \* Objects fill the magazines in order: object o sits in magazine
    \* (o-1) \div Cap + 1.
    /\ slot = [m \in Mags |-> [i \in 1..Cap |->
                  IF (m - 1) * Cap + i <= NObjs THEN (m - 1) * Cap + i ELSE 0]]
    /\ pool = {Word(m, 1 + Cardinality({o \in Objs : (o - 1) \div Cap + 1 = m})) : m \in Mags}
    /\ back = {}
    /\ user = [t \in Threads |-> {}]
    /\ pc = [t \in Threads |-> "idle"]
    /\ rc = [t \in Threads |-> 0]
    /\ rw = [t \in Threads |-> Zero]
    /\ ro = [t \in Threads |-> 0]
    /\ obj = [t \in Threads |-> 0]
    /\ hw = [t \in Threads |-> Zero]

Running(t) == cid[t] # 0

Goto(t, l) == pc' = [pc EXCEPT ![t] = l]
ClearRegs(t) ==
    /\ rc' = [rc EXCEPT ![t] = 0]
    /\ rw' = [rw EXCEPT ![t] = Zero]
    /\ ro' = [ro EXCEPT ![t] = 0]

----------------------------------------------------------------------------
(* The program *)

StartAlloc(t) ==
    /\ pc[t] = "idle"
    /\ Goto(t, "pop_cid")
    /\ UNCHANGED <<cid, slab, slot, pool, back, user, rc, rw, ro, obj, hw>>

StartFree(t) ==
    /\ pc[t] = "idle"
    /\ \E o \in user[t] :
        /\ obj' = [obj EXCEPT ![t] = o]
        /\ user' = [user EXCEPT ![t] = @ \ {o}]
    /\ Goto(t, "push_cid")
    /\ UNCHANGED <<cid, slab, slot, pool, back, rc, rw, ro, hw>>

----------------------------------------------------------------------------
(* The routines. Each label is one instruction that touches memory. *)

\* Read the cid and compare it with the slab count. Out of range: pop and
\* push leave for the back end; install gives its magazine back first.
ReadCid(t, from, to) ==
    /\ pc[t] = from
    /\ IF cid[t] \in Cids
       THEN rc' = [rc EXCEPT ![t] = cid[t]] /\ Goto(t, to)
       ELSE UNCHANGED rc /\ Goto(t, IF from = "ins_cid" THEN "put_back" ELSE "direct")
    /\ UNCHANGED <<cid, slab, slot, pool, back, user, rw, ro, obj, hw>>

PopLoad(t) ==
    /\ pc[t] = "pop_load"
    /\ LET w == slab[rc[t]] IN
        IF IsEmpty(w)
        THEN /\ Goto(t, "a_get") /\ ClearRegs(t)
        ELSE /\ Goto(t, "pop_read")
             /\ rw' = [rw EXCEPT ![t] = w]
             /\ UNCHANGED <<rc, ro>>
    /\ UNCHANGED <<cid, slab, slot, pool, back, user, obj, hw>>

PopRead(t) ==
    /\ pc[t] = "pop_read"
    /\ ro' = [ro EXCEPT ![t] = slot[rw[t].m][rw[t].s - 1]]
    /\ Goto(t, "pop_commit")
    /\ UNCHANGED <<cid, slab, slot, pool, back, user, rc, rw, obj, hw>>

PopCommit(t) ==
    /\ pc[t] = "pop_commit"
    /\ slab' = [slab EXCEPT ![rc[t]] = Word(rw[t].m, rw[t].s - 1)]
    /\ user' = [user EXCEPT ![t] = @ \cup {ro[t]}]
    /\ Goto(t, "idle") /\ ClearRegs(t)
    /\ UNCHANGED <<cid, slot, pool, back, obj, hw>>

PushLoad(t) ==
    /\ pc[t] = "push_load"
    /\ LET w == slab[rc[t]] IN
        IF IsFull(w)
        THEN /\ Goto(t, "f_get") /\ ClearRegs(t)
        ELSE /\ Goto(t, "push_store")
             /\ rw' = [rw EXCEPT ![t] = w]
             /\ UNCHANGED <<rc, ro>>
    /\ UNCHANGED <<cid, slab, slot, pool, back, user, obj, hw>>

\* The store into the free slot happens before the commit. The slab invariant
\* says nothing about slots at or above the word, so this is invisible.
PushStore(t) ==
    /\ pc[t] = "push_store"
    /\ slot' = [slot EXCEPT ![rw[t].m][rw[t].s] = obj[t]]
    /\ Goto(t, "push_commit")
    /\ UNCHANGED <<cid, slab, pool, back, user, rc, rw, ro, obj, hw>>

PushCommit(t) ==
    /\ pc[t] = "push_commit"
    /\ slab' = [slab EXCEPT ![rc[t]] = Word(rw[t].m, rw[t].s + 1)]
    /\ obj' = [obj EXCEPT ![t] = 0]
    /\ Goto(t, "idle") /\ ClearRegs(t)
    /\ UNCHANGED <<cid, slot, pool, back, user, hw>>

Allocating(t) == obj[t] = 0
Retry(t) == IF Allocating(t) THEN "pop_cid" ELSE "push_cid"

\* Install the held magazine if the slab word shows none, or one that is
\* empty (when allocating) or full (when freeing). Returns the old word.
InsLoad(t) ==
    /\ pc[t] = "ins_load"
    /\ LET w == slab[rc[t]]
           ok == IF Allocating(t) THEN IsEmpty(w) ELSE IsFull(w) IN
        IF ok
        THEN /\ Goto(t, "ins_commit")
             /\ rw' = [rw EXCEPT ![t] = w]
             /\ UNCHANGED <<rc, ro>>
        ELSE /\ Goto(t, "put_back") /\ ClearRegs(t)
    /\ UNCHANGED <<cid, slab, slot, pool, back, user, obj, hw>>

InsCommit(t) ==
    /\ pc[t] = "ins_commit"
    /\ slab' = [slab EXCEPT ![rc[t]] = hw[t]]
    /\ hw' = [hw EXCEPT ![t] = rw[t]]
    /\ Goto(t, "retire") /\ ClearRegs(t)
    /\ UNCHANGED <<cid, slot, pool, back, user, obj>>

----------------------------------------------------------------------------
(* The rest of the slow path, abstracted to atomic steps on a pool. *)

Get(t) ==
    /\ \/ /\ pc[t] = "a_get"
          /\ \E w \in pool : ~IsEmpty(w) /\ hw' = [hw EXCEPT ![t] = w] /\ pool' = pool \ {w}
          /\ Goto(t, "ins_cid")
          /\ UNCHANGED user
       \/ /\ pc[t] = "a_get"          \* nothing to allocate: fail the request
          /\ \A w \in pool : IsEmpty(w)
          /\ Goto(t, "idle")
          /\ UNCHANGED <<hw, pool, user>>
       \/ /\ pc[t] = "f_get"
          /\ \E w \in pool : ~IsFull(w) /\ hw' = [hw EXCEPT ![t] = w] /\ pool' = pool \ {w}
          /\ Goto(t, "ins_cid")
          /\ UNCHANGED user
    /\ UNCHANGED <<cid, slab, slot, back, rc, rw, ro, obj>>

\* Retire the magazine the install displaced, or put back the one it could
\* not install, then retry the request.
Return(t) ==
    /\ pc[t] \in {"retire", "put_back"}
    /\ pool' = IF hw[t] = Zero THEN pool ELSE pool \cup {hw[t]}
    /\ hw' = [hw EXCEPT ![t] = Zero]
    /\ Goto(t, Retry(t))
    /\ UNCHANGED <<cid, slab, slot, back, user, rc, rw, ro, obj>>

\* With no usable cid, serve the call from the back end.
Direct(t) ==
    /\ pc[t] = "direct"
    /\ IF Allocating(t)
       THEN IF back = {} THEN UNCHANGED <<back, user, obj>>
            ELSE \E o \in back :
                    /\ back' = back \ {o}
                    /\ user' = [user EXCEPT ![t] = @ \cup {o}]
                    /\ UNCHANGED obj
       ELSE /\ back' = back \cup {obj[t]}
            /\ obj' = [obj EXCEPT ![t] = 0]
            /\ UNCHANGED user
    /\ Goto(t, "idle")
    /\ UNCHANGED <<cid, slab, slot, pool, rc, rw, ro, hw>>

----------------------------------------------------------------------------
(* The OS *)

SeqStart(l) ==
    CASE l \in {"pop_load", "pop_read", "pop_commit"}     -> "pop_cid"
      [] l \in {"push_load", "push_store", "push_commit"} -> "push_cid"
      [] l \in {"ins_load", "ins_commit"}                 -> "ins_cid"
      [] OTHER -> l

\* Stop a thread. If it was inside a sequence it will resume at the abort
\* handler, which is the same as resetting it now: nothing can observe a
\* stopped thread's registers.
Preempt(t) ==
    /\ Running(t)
    /\ cid' = [cid EXCEPT ![t] = 0]
    /\ IF Restart /\ SeqStart(pc[t]) # pc[t]
       THEN Goto(t, SeqStart(pc[t])) /\ ClearRegs(t)
       ELSE UNCHANGED <<pc, rc, rw, ro>>
    /\ UNCHANGED <<slab, slot, pool, back, user, obj, hw>>

Resume(t) ==
    /\ ~Running(t)
    /\ \E c \in Cids \cup (IF BadCid THEN {NCids + 1} ELSE {}) :
        /\ (Exclusive /\ c \in Cids) => \A u \in Threads : cid[u] # c
        /\ cid' = [cid EXCEPT ![t] = c]
    /\ UNCHANGED <<slab, slot, pool, back, user, pc, rc, rw, ro, obj, hw>>

Step(t) ==
    /\ Running(t)
    /\ \/ StartAlloc(t) \/ StartFree(t)
       \/ ReadCid(t, "pop_cid", "pop_load") \/ PopLoad(t) \/ PopRead(t) \/ PopCommit(t)
       \/ ReadCid(t, "push_cid", "push_load") \/ PushLoad(t) \/ PushStore(t) \/ PushCommit(t)
       \/ ReadCid(t, "ins_cid", "ins_load") \/ InsLoad(t) \/ InsCommit(t)
       \/ Get(t) \/ Return(t) \/ Direct(t)

Next == \E t \in Threads : Step(t) \/ Preempt(t) \/ Resume(t)

Spec == Init /\ [][Next]_vars

Symm == Permutations(Threads)

----------------------------------------------------------------------------

\* The objects a word accounts for: slots 1 to s-1 of its magazine.
Below(w) == IF w.m = 0 THEN {} ELSE {slot[w.m][i] : i \in 1..(w.s - 1)}

\* Every place a magazine can be owned from, with the word that names it.
OwnedWords == {<<"slab", c, slab[c]>> : c \in Cids}
         \cup {<<"held", t, hw[t]>> : t \in Threads}
         \cup {<<"pool", 0, w>> : w \in pool}

Places(o) == {p \in OwnedWords : o \in Below(p[3])}
        \cup (IF o \in back THEN {<<"back">>} ELSE {})
        \cup {<<"user", t>> : t \in {u \in Threads : o \in user[u]}}
        \cup {<<"freeing", t>> : t \in {u \in Threads : obj[u] = o}}

CidsDistinct ==
    Exclusive => \A t, u \in Threads : t # u /\ cid[t] \in Cids => cid[t] # cid[u]

\* The slab invariant from "Front end", extended to every place a magazine
\* can be: each word is zero or names slot s of a magazine nothing else refers
\* to, with slots 1..s-1 holding distinct objects that appear nowhere else.
SlabInvariant ==
    /\ \A m \in Mags : Cardinality({p \in OwnedWords : p[3].m = m}) = 1
    /\ \A p \in OwnedWords :
        p[3] # Zero =>
            /\ p[3].s \in 1..(Slots - 1)
            /\ 0 \notin Below(p[3])
            /\ Cardinality(Below(p[3])) = p[3].s - 1

\* A sequence that got past the range check holds an in-range cid, so a
\* thread with no valid cid never reads or writes a slab.
InRange == \A t \in Threads : SeqStart(pc[t]) # pc[t] => rc[t] \in Cids

\* No object is in two places and none is lost.
Conservation == \A o \in Objs : Cardinality(Places(o)) = 1

=============================================================================
