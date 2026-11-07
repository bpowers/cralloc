-------------------------------- MODULE Fork --------------------------------
(* Fork: a copy of the whole shared state taken at an arbitrary step, with
   only the forking thread surviving.

   This model puts one small instance of every shared structure side by side:
   slab words with their drain flags, magazines, the depot, one free-bitmap
   word with its summary bit, and the purge claim. Routines and depot
   operations are single steps (Rseq.tla, Depot.tla); everything that takes
   several atomic instructions is several steps, so the fork can land between
   any two of them.

   After the fork the other threads are reset to fresh ones, which stands for
   threads the child creates later. `lostO` and `lostM` record what no
   survivor can reach at that instant, and `predO` and `predM` what the
   vanished threads held privately. The model checks that the two are equal
   (the leak is exactly the private holdings), that nothing else is ever lost
   or duplicated in the child, and which hints and claims are left stuck.

   Repair:
     "none"   nothing is done about a drain cut off by the fork.
     "sweep"  rejected: the first slow path in the child clears every
              marker and flag; no drain starts until the sweep is done.
     "gen"    the design: flags and markers carry the fork generation, and a
              drain takes over a stale claim (Drain.tla).
   Heal = TRUE is the design's healing free, as in Summary.tla.
   ForkMode "idle": the forking thread is outside the allocator. "any": it
   may be at any step (fork called from a signal handler). *)
EXTENDS Naturals, FiniteSets, TLC

CONSTANTS Threads, Cids, Mags, Objs, Cap, Repair, Heal, ForkMode

Zero == <<"zero", 0>>
NoObj == "none"
Marker(g) == <<"marker", g>>
IsMark(w) == w \notin Mags /\ w # Zero
IsMag(w) == w \in Mags

VARIABLES
    slab,    \* slab[c]: Zero, a marker, or the loaded magazine
    flag,    \* flag[c]: the drain claim
    mag,     \* mag[m]: objects in magazine m
    depot,   \* magazines in the depot
    freeb,   \* the free-bitmap word: objects whose bit is set
    summ,    \* its summary bit
    user,    \* objects the program holds
    th,
    gen, wipe, forked,
    lostO, lostM, predO, predM

vars == <<slab, flag, mag, depot, freeb, summ, user, th, gen, wipe, forked,
          lostO, lostM, predO, predM>>

Idle == [pc |-> "idle", ret |-> "idle",
         o |-> NoObj,      \* the object being freed
         hm |-> Zero,      \* a magazine held privately
         cl |-> {},        \* bits claimed from, or on their way to, the bitmap
         snap |-> {},      \* the bitmap word as read
         dc |-> Zero, old |-> Zero, g |-> 0]    \* drain

Init ==
    /\ slab = [c \in Cids |-> Zero]
    /\ flag = [c \in Cids |-> 0]
    /\ mag = [m \in Mags |-> {}]
    /\ depot = Mags
    /\ freeb = Objs
    /\ summ = TRUE
    /\ user = {}
    /\ th = [t \in Threads |-> Idle]
    /\ gen = 1
    /\ wipe = 2
    /\ forked = FALSE
    /\ lostO = {} /\ lostM = {} /\ predO = {} /\ predM = {}

Set(t, T) == th' = [th EXCEPT ![t] = T]

\* The objects in the magazine loaded on cid c, and whether it has room.
Loaded(c) == IF IsMag(slab[c]) THEN mag[slab[c]] ELSE {}
Room(c) == IF IsMag(slab[c]) THEN Cardinality(mag[slab[c]]) < Cap ELSE FALSE
At(t, l) == th[t].pc = l

----------------------------------------------------------------------------
(* Back end: one bitmap word and its summary bit, as in Summary.tla *)

\* Set bits with one atomic OR; set the summary bit if the word was empty.
\* With Heal, look at the summary bit regardless.
BitsOr(t) ==
    /\ At(t, "b_or")
    /\ LET T == th[t] IN
        /\ freeb' = freeb \cup T.cl
        /\ Set(t, [T EXCEPT !.cl = {},
                            !.pc = IF freeb = {} THEN "b_set"
                                   ELSE IF Heal THEN "b_chk" ELSE T.ret])
    /\ UNCHANGED <<slab, flag, mag, depot, summ, user, gen, wipe, forked, lostO, lostM, predO, predM>>

BitsCheck(t) ==
    /\ At(t, "b_chk")
    /\ Set(t, [th[t] EXCEPT !.pc = IF summ THEN th[t].ret ELSE "b_set"])
    /\ UNCHANGED <<slab, flag, mag, depot, freeb, summ, user, gen, wipe, forked, lostO, lostM, predO, predM>>

SummSet(t) ==
    /\ At(t, "b_set")
    /\ summ' = TRUE
    /\ Set(t, [th[t] EXCEPT !.pc = th[t].ret])
    /\ UNCHANGED <<slab, flag, mag, depot, freeb, user, gen, wipe, forked, lostO, lostM, predO, predM>>

\* Whoever empties the word clears the summary bit, then reads the word again.
SummClear(t) ==
    /\ At(t, "b_clr")
    /\ summ' = FALSE
    /\ Set(t, [th[t] EXCEPT !.pc = "b_again"])
    /\ UNCHANGED <<slab, flag, mag, depot, freeb, user, gen, wipe, forked, lostO, lostM, predO, predM>>

ReadAgain(t) ==
    /\ At(t, "b_again")
    /\ Set(t, [th[t] EXCEPT !.pc = IF freeb # {} THEN "b_set" ELSE th[t].ret])
    /\ UNCHANGED <<slab, flag, mag, depot, freeb, summ, user, gen, wipe, forked, lostO, lostM, predO, predM>>

\* Search: read the summary bit and the word.
FillRead(t) ==
    /\ At(t, "s_read")
    /\ LET T == th[t] IN
        IF ~summ THEN Set(t, [T EXCEPT !.pc = "a_load"])
        ELSE IF freeb = {} THEN Set(t, [T EXCEPT !.pc = "b_clr", !.ret = "a_load"])
        ELSE Set(t, [T EXCEPT !.pc = "s_cas", !.snap = freeb])
    /\ UNCHANGED <<slab, flag, mag, depot, freeb, summ, user, gen, wipe, forked, lostO, lostM, predO, predM>>

\* Claim bits with one compare-and-swap.
FillCas(t) ==
    /\ At(t, "s_cas")
    /\ LET T == th[t] IN
        IF freeb = T.snap
        THEN \E S \in SUBSET T.snap \ {{}} :
                /\ Cardinality(S) <= Cap
                /\ freeb' = freeb \ S
                /\ Set(t, [T EXCEPT !.cl = S, !.snap = {}, !.ret = "a_load",
                                    !.pc = IF S = T.snap THEN "b_clr" ELSE "a_load"])
        ELSE Set(t, [T EXCEPT !.snap = {}, !.pc = "s_read"]) /\ UNCHANGED freeb
    /\ UNCHANGED <<slab, flag, mag, depot, summ, user, gen, wipe, forked, lostO, lostM, predO, predM>>

----------------------------------------------------------------------------
(* Allocation *)

AFast(t) ==
    /\ At(t, "idle")
    /\ \/ \E c \in Cids : \E o \in Loaded(c) :
            /\ mag' = [mag EXCEPT ![slab[c]] = @ \ {o}]
            /\ user' = user \cup {o}
            /\ UNCHANGED th
       \/ /\ \E c \in Cids : Loaded(c) = {}
          /\ Set(t, [Idle EXCEPT !.pc = "a_get"])
          /\ UNCHANGED <<mag, user>>
    /\ UNCHANGED <<slab, flag, depot, freeb, summ, gen, wipe, forked, lostO, lostM, predO, predM>>

\* A non-empty magazine from the depot, or an empty one to fill.
AGet(t) ==
    /\ At(t, "a_get")
    /\ wipe # 0
    /\ IF depot = {} THEN Set(t, Idle) /\ UNCHANGED depot
       ELSE \E m \in depot :
            /\ depot' = depot \ {m}
            /\ Set(t, [th[t] EXCEPT !.hm = m, !.pc = IF mag[m] = {} THEN "s_read" ELSE "a_ins"])
    /\ UNCHANGED <<slab, flag, mag, freeb, summ, user, gen, wipe, forked, lostO, lostM, predO, predM>>

\* Put the claimed objects into the held magazine. With none, give up.
ALoad(t) ==
    /\ At(t, "a_load")
    /\ LET T == th[t] IN
        /\ mag' = [mag EXCEPT ![T.hm] = T.cl]
        /\ Set(t, [T EXCEPT !.cl = {}, !.ret = "idle",
                            !.pc = IF T.cl = {} THEN "to_depot" ELSE "a_ins"])
    /\ UNCHANGED <<slab, flag, depot, freeb, summ, user, gen, wipe, forked, lostO, lostM, predO, predM>>

AInstall(t) ==
    /\ At(t, "a_ins")
    /\ LET T == th[t] IN
       \E c \in Cids :
        IF IsMark(slab[c])
        THEN \E o \in mag[T.hm] :
                /\ mag' = [mag EXCEPT ![T.hm] = @ \ {o}]
                /\ user' = user \cup {o}
                /\ Set(t, [T EXCEPT !.pc = "to_depot"])
                /\ UNCHANGED slab
        ELSE IF Loaded(c) = {}
        THEN /\ slab' = [slab EXCEPT ![c] = T.hm]
             /\ Set(t, [T EXCEPT !.hm = slab[c], !.pc = "to_depot"])
             /\ UNCHANGED <<mag, user>>
        ELSE Set(t, [T EXCEPT !.pc = "to_depot"]) /\ UNCHANGED <<slab, mag, user>>
    /\ UNCHANGED <<flag, depot, freeb, summ, gen, wipe, forked, lostO, lostM, predO, predM>>

\* Send the held magazine, if any, to the depot. A free that has not yet
\* placed its object retries.
ToDepot(t) ==
    /\ At(t, "to_depot")
    /\ LET T == th[t] IN
        /\ depot' = IF T.hm = Zero THEN depot ELSE depot \cup {T.hm}
        /\ Set(t, IF T.o = NoObj THEN Idle ELSE [Idle EXCEPT !.o = T.o, !.pc = "f_fast"])
    /\ UNCHANGED <<slab, flag, mag, freeb, summ, user, gen, wipe, forked, lostO, lostM, predO, predM>>

----------------------------------------------------------------------------
(* Free *)

FreeBegin(t) ==
    /\ At(t, "idle")
    /\ \E o \in user :
        /\ user' = user \ {o}
        /\ Set(t, [Idle EXCEPT !.o = o, !.pc = "f_fast"])
    /\ UNCHANGED <<slab, flag, mag, depot, freeb, summ, gen, wipe, forked, lostO, lostM, predO, predM>>

FFast(t) ==
    /\ At(t, "f_fast")
    /\ \/ \E c \in Cids :
            /\ Room(c)
            /\ mag' = [mag EXCEPT ![slab[c]] = @ \cup {th[t].o}]
            /\ Set(t, Idle)
       \/ /\ \E c \in Cids : ~Room(c)
          /\ Set(t, [th[t] EXCEPT !.pc = "f_get"])
          /\ UNCHANGED mag
    /\ UNCHANGED <<slab, flag, depot, freeb, summ, user, gen, wipe, forked, lostO, lostM, predO, predM>>

\* An empty magazine from the depot; with none, free straight to the bitmap.
FGet(t) ==
    /\ At(t, "f_get")
    /\ wipe # 0
    /\ LET T == th[t] IN
        IF \E m \in depot : mag[m] = {}
        THEN \E m \in depot :
                /\ mag[m] = {}
                /\ depot' = depot \ {m}
                /\ Set(t, [T EXCEPT !.hm = m, !.pc = "f_ins"])
        ELSE /\ Set(t, [T EXCEPT !.o = NoObj, !.cl = {T.o}, !.pc = "b_or", !.ret = "idle"])
             /\ UNCHANGED depot
    /\ UNCHANGED <<slab, flag, mag, freeb, summ, user, gen, wipe, forked, lostO, lostM, predO, predM>>

FInstall(t) ==
    /\ At(t, "f_ins")
    /\ LET T == th[t] IN
       \E c \in Cids :
        IF IsMark(slab[c])
        THEN /\ mag' = [mag EXCEPT ![T.hm] = @ \cup {T.o}]
             /\ Set(t, [T EXCEPT !.o = NoObj, !.pc = "to_depot"])
             /\ UNCHANGED slab
        ELSE IF ~Room(c)
        THEN /\ slab' = [slab EXCEPT ![c] = T.hm]
             /\ Set(t, [T EXCEPT !.hm = slab[c], !.pc = "f_retire"])
             /\ UNCHANGED mag
        ELSE Set(t, [T EXCEPT !.pc = "to_depot"]) /\ UNCHANGED <<slab, mag>>
    /\ UNCHANGED <<flag, depot, freeb, summ, user, gen, wipe, forked, lostO, lostM, predO, predM>>

\* The displaced full magazine goes to the depot, or is laundered first.
FRetire(t) ==
    /\ At(t, "f_retire")
    /\ LET T == th[t] IN
        \/ Set(t, [T EXCEPT !.pc = "to_depot"]) /\ UNCHANGED mag
        \/ /\ T.hm # Zero
           /\ mag' = [mag EXCEPT ![T.hm] = {}]
           /\ Set(t, [T EXCEPT !.cl = mag[T.hm], !.pc = "b_or", !.ret = "to_depot"])
    /\ UNCHANGED <<slab, flag, depot, freeb, summ, user, gen, wipe, forked, lostO, lostM, predO, predM>>

----------------------------------------------------------------------------
(* Purge: claim the whole word if it is fully free, return the memory,
   release. *)

PurgeClaim(t) ==
    /\ At(t, "idle")
    /\ wipe # 0
    /\ freeb = Objs
    /\ freeb' = {}
    /\ Set(t, [Idle EXCEPT !.cl = Objs, !.pc = "b_clr", !.ret = "p_madv"])
    /\ UNCHANGED <<slab, flag, mag, depot, summ, user, gen, wipe, forked, lostO, lostM, predO, predM>>

PurgeMadvise(t) ==
    /\ At(t, "p_madv")
    /\ Set(t, [th[t] EXCEPT !.pc = "b_or", !.ret = "idle"])
    /\ UNCHANGED <<slab, flag, mag, depot, freeb, summ, user, gen, wipe, forked, lostO, lostM, predO, predM>>

----------------------------------------------------------------------------
(* Drain, as in Drain.tla but with no sequences in flight *)

Claimable(f) == f = 0 \/ (Repair = "gen" /\ f < gen)
Token == IF Repair = "gen" THEN gen ELSE 1

DClaim(t) ==
    /\ At(t, "idle")
    /\ wipe = 2
    /\ \E c \in Cids :
        /\ Claimable(flag[c])
        /\ flag' = [flag EXCEPT ![c] = Token]
        /\ Set(t, [Idle EXCEPT !.dc = c, !.g = Token, !.pc = "d_x"])
    /\ UNCHANGED <<slab, mag, depot, freeb, summ, user, gen, wipe, forked, lostO, lostM, predO, predM>>

DExchange(t) ==
    /\ At(t, "d_x")
    /\ slab' = [slab EXCEPT ![th[t].dc] = Marker(th[t].g)]
    /\ Set(t, [th[t] EXCEPT !.old = slab[th[t].dc], !.pc = "d_r"])
    /\ UNCHANGED <<flag, mag, depot, freeb, summ, user, gen, wipe, forked, lostO, lostM, predO, predM>>

DReread(t) ==
    /\ At(t, "d_r")
    /\ LET T == th[t] IN
        IF slab[T.dc] = Marker(T.g)
        THEN Set(t, [T EXCEPT !.hm = IF IsMag(T.old) THEN T.old ELSE Zero, !.old = Zero, !.pc = "d_z"])
        ELSE Set(t, [T EXCEPT !.old = Zero, !.pc = "d_rel"])
    /\ UNCHANGED <<slab, flag, mag, depot, freeb, summ, user, gen, wipe, forked, lostO, lostM, predO, predM>>

DZero(t) ==
    /\ At(t, "d_z")
    /\ slab' = [slab EXCEPT ![th[t].dc] = Zero]
    /\ depot' = IF th[t].hm = Zero THEN depot ELSE depot \cup {th[t].hm}
    /\ Set(t, [th[t] EXCEPT !.hm = Zero, !.pc = "d_rel"])
    /\ UNCHANGED <<flag, mag, freeb, summ, user, gen, wipe, forked, lostO, lostM, predO, predM>>

DRelease(t) ==
    /\ At(t, "d_rel")
    /\ flag' = IF Repair = "gen" /\ flag[th[t].dc] # th[t].g THEN flag
               ELSE [flag EXCEPT ![th[t].dc] = 0]
    /\ Set(t, Idle)
    /\ UNCHANGED <<slab, mag, depot, freeb, summ, user, gen, wipe, forked, lostO, lostM, predO, predM>>

\* The sweep: one step per slab, then mark the page.
SweepBegin(t) ==
    /\ wipe = 0
    /\ th[t].pc \in {"idle", "a_get", "f_get"}
    /\ wipe' = 1
    /\ Set(t, [th[t] EXCEPT !.ret = th[t].pc, !.pc = "r_sweep", !.snap = Cids])
    /\ UNCHANGED <<slab, flag, mag, depot, freeb, summ, user, gen, forked, lostO, lostM, predO, predM>>

Sweep(t) ==
    /\ At(t, "r_sweep")
    /\ LET T == th[t] IN
        IF T.snap = {}
        THEN /\ wipe' = 2
             /\ Set(t, [T EXCEPT !.pc = T.ret, !.ret = "idle"])
             /\ UNCHANGED <<slab, flag>>
        ELSE \E c \in T.snap :
             /\ slab' = [slab EXCEPT ![c] = IF IsMark(@) THEN Zero ELSE @]
             /\ flag' = [flag EXCEPT ![c] = 0]
             /\ Set(t, [T EXCEPT !.snap = @ \ {c}])
             /\ UNCHANGED wipe
    /\ UNCHANGED <<mag, depot, freeb, summ, user, gen, forked, lostO, lostM, predO, predM>>

----------------------------------------------------------------------------
(* Fork *)

MagPlaces(m, thr, sl, dp) ==
       {<<"slab", c>> : c \in {c \in Cids : sl[c] = m}}
  \cup (IF m \in dp THEN {<<"depot">>} ELSE {})
  \cup {<<"held", t>> : t \in {t \in Threads : thr[t].hm = m}}
  \cup {<<"drained", t>> : t \in {t \in Threads : thr[t].old = m}}

ObjPlaces(o, thr) ==
       {<<"mag", m>> : m \in {m \in Mags : o \in mag[m]}}
  \cup (IF o \in freeb THEN {<<"free">>} ELSE {})
  \cup (IF o \in user THEN {<<"user">>} ELSE {})
  \cup {<<"freeing", t>> : t \in {t \in Threads : thr[t].o = o}}
  \cup {<<"claimed", t>> : t \in {t \in Threads : o \in thr[t].cl}}

\* What a thread holds that nothing shared refers to.
PrivMags(T) == {T.hm, T.old} \cap Mags
PrivObjs(T) == ({T.o} \cap Objs) \cup T.cl \cup UNION {mag[m] : m \in PrivMags(T)}

Fork(t) ==
    /\ ForkMode # "none" /\ ~forked
    /\ ForkMode = "idle" => At(t, "idle")
    /\ forked' = TRUE
    /\ gen' = gen + 1
    /\ wipe' = IF Repair = "sweep" THEN 0 ELSE wipe
    /\ th' = [u \in Threads |-> IF u = t THEN th[t] ELSE Idle]
    /\ lostM' = {m \in Mags : MagPlaces(m, th', slab, depot) = {}}
    /\ lostO' = {o \in Objs : ObjPlaces(o, th') = {} \/ \E m \in lostM' : o \in mag[m]}
    /\ predM' = UNION {PrivMags(th[u]) : u \in Threads \ {t}}
    /\ predO' = UNION {PrivObjs(th[u]) : u \in Threads \ {t}}
    /\ UNCHANGED <<slab, flag, mag, depot, freeb, summ, user>>

Next == \E t \in Threads :
    \/ BitsOr(t) \/ BitsCheck(t) \/ SummSet(t) \/ SummClear(t) \/ ReadAgain(t)
    \/ FillRead(t) \/ FillCas(t)
    \/ AFast(t) \/ AGet(t) \/ ALoad(t) \/ AInstall(t) \/ ToDepot(t)
    \/ FreeBegin(t) \/ FFast(t) \/ FGet(t) \/ FInstall(t) \/ FRetire(t)
    \/ PurgeClaim(t) \/ PurgeMadvise(t)
    \/ DClaim(t) \/ DExchange(t) \/ DReread(t) \/ DZero(t) \/ DRelease(t)
    \/ SweepBegin(t) \/ Sweep(t)
    \/ Fork(t)

Spec == Init /\ [][Next]_vars

Symm == Permutations(Threads) \cup Permutations(Cids) \cup Permutations(Mags) \cup Permutations(Objs)

----------------------------------------------------------------------------

\* Nothing is in two places, in the parent or the child.
NoDouble ==
    /\ \A m \in Mags : Cardinality(MagPlaces(m, th, slab, depot)) <= 1
    /\ \A o \in Objs : Cardinality(ObjPlaces(o, th)) <= 1
    /\ \A m \in Mags : Cardinality(mag[m]) <= Cap

\* Nothing is lost except what the fork took.
NoLoss ==
    /\ \A m \in Mags : MagPlaces(m, th, slab, depot) # {} \/ m \in lostM
    /\ \A o \in Objs : ObjPlaces(o, th) # {} \/ o \in lostO

\* What the child cannot reach is exactly what the vanished threads held
\* privately: at most one magazine each, with its objects, plus one object
\* being freed, or the bits of one bitmap word being claimed, laundered or
\* purged.
LeakIsPrivate == lostM = predM /\ lostO = predO
OneMagEach == \A t \in Threads : Cardinality(PrivMags(th[t])) <= 1

Quiet == \A t \in Threads : th[t].pc = "idle"

\* Free objects can be found from the summary bit.
Visible == Quiet => (freeb # {} => summ)

\* No marker or drain flag outlives its drain.
Stale(k) == Repair = "gen" /\ k < gen
Clean ==
    (Quiet /\ wipe = 2) =>
        \A c \in Cids :
            /\ IsMark(slab[c]) => Stale(slab[c][2])
            /\ flag[c] # 0 => Stale(flag[c])

=============================================================================
