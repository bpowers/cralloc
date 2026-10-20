------------------------------ MODULE Exchange ------------------------------
(* The slow path: exchanging magazines.

   Each routine (pop, push, install, spare push, spare pop) is one atomic step
   here; Rseq.tla shows that a routine either commits as one step or leaves no
   trace. What this model explores is everything between the routines:

     migration     every routine step runs on an arbitrary cid, so a thread
                   may be on a different cid at each step, and two threads
                   may use one cid in turn;
     signals       a handler is a nested call on the same thread, pushed at
                   any step of the interrupted one. The abort that precedes
                   it is implicit: no routine is ever half done;
     the marker    an abstract drainer marks a cid's slab word and spare
                   heads, and later takes what it displaced or backs out;
     no cid        a routine may find the thread's cid out of range. The call
                   is then served from the back end and touches no slab;
     centring      a swap may leave its magazine half full instead (slow.rs,
                   "Half full on a reversal"): it takes the loaded magazine
                   out, storing Zero, copies part of one held magazine into
                   the other, and installs one expecting Zero. Any swap may
                   do it here, since the rule that picks them is a hint.

   Magazines hold sets of objects. The depot and the back end are atomic
   here; Depot.tla, Summary.tla and Purge.tla model them. *)
EXTENDS Naturals, FiniteSets, Sequences, TLC

CONSTANTS
    Threads,
    Cids,
    Mags,
    Objs,
    Cap,        \* objects per magazine
    MaxDepth,   \* calls in progress per thread: 1 is no signal handler
    SpareMax,   \* spare magazines of each kind a cid may keep
    BadCid,     \* TRUE: a thread may find its cid out of range
    BlindCentre \* TRUE: a centring installs without expecting Zero (a flaw)

\* Cids, magazines and objects are sets of model values, so TLC can treat
\* them as interchangeable.
NoCid == "nocid"
NoObj == "none"
Zero == "zero"       \* slab word: no magazine
Marker == "marker"

VARIABLES
    slab,     \* slab[c]: Zero, Marker, or the loaded magazine
    spFull,   \* spFull[c]: the cid's spare non-empty magazines
    spEmpty,  \* spEmpty[c]: the cid's spare empty magazines
    spMark,   \* spMark[c]: the spare heads hold the marker
    drained,  \* drained[c]: magazines the drainer displaced and may take
    depot,    \* magazines in the depot
    mag,      \* mag[m]: the objects in magazine m
    back,     \* objects free in the back end
    user,     \* objects the program holds
    stack     \* stack[t]: calls in progress, innermost last

vars == <<slab, spFull, spEmpty, spMark, drained, depot, mag, back, user, stack>>

Init ==
    /\ slab = [c \in Cids |-> Zero]
    /\ spFull = [c \in Cids |-> {}]
    /\ spEmpty = [c \in Cids |-> {}]
    /\ spMark = [c \in Cids |-> FALSE]
    /\ drained = [c \in Cids |-> {}]
    /\ depot = Mags
    /\ mag = [m \in Mags |-> {}]
    /\ back = Objs
    /\ user = {}
    /\ stack = [t \in Threads |-> <<>>]

Depth(t) == Len(stack[t])
Top(t) == stack[t][Depth(t)]
SetTop(t, f) == stack' = [stack EXCEPT ![t][Depth(t)] = f]
Goto(t, l) == SetTop(t, [Top(t) EXCEPT !.pc = l])
Return(t) == stack' = [stack EXCEPT ![t] = SubSeq(@, 1, Depth(t) - 1)]
At(t, l) == Depth(t) > 0 /\ Top(t).pc = l

\* The cid a routine finds itself on.
SomeCid == Cids \cup (IF BadCid THEN {NoCid} ELSE {})

IsMag(w) == w \in Mags
\* hm: the magazine the call holds; om: the loaded one a centring took out.
Frame(op, o) == [op |-> op, pc |-> IF op = "alloc" THEN "a_fast" ELSE "f_fast",
                 o |-> o, hm |-> Zero, om |-> Zero]

----------------------------------------------------------------------------
(* Calls, from the program or from a signal handler *)

Call(t) ==
    /\ Depth(t) < MaxDepth
    /\ \/ /\ stack' = [stack EXCEPT ![t] = Append(@, Frame("alloc", NoObj))]
          /\ UNCHANGED user
       \/ \E o \in user :
          /\ user' = user \ {o}
          /\ stack' = [stack EXCEPT ![t] = Append(@, Frame("free", o))]
    /\ UNCHANGED <<slab, spFull, spEmpty, spMark, drained, depot, mag, back>>

----------------------------------------------------------------------------
(* Allocation *)

\* The pop routine, or the range check that sends an out-of-range cid to the
\* back end.
AFast(t) ==
    /\ At(t, "a_fast")
    /\ \E c \in SomeCid :
        IF c = NoCid THEN Goto(t, "a_direct") /\ UNCHANGED <<mag, user>>
        ELSE IF IsMag(slab[c]) /\ mag[slab[c]] # {}
        THEN \E o \in mag[slab[c]] :
                /\ mag' = [mag EXCEPT ![slab[c]] = @ \ {o}]
                /\ user' = user \cup {o}
                /\ Return(t)
        ELSE Goto(t, "a_get") /\ UNCHANGED <<mag, user>>
    /\ UNCHANGED <<slab, spFull, spEmpty, spMark, drained, depot, back>>

\* Serve straight from the back end, or fail if it is empty.
ADirect(t) ==
    /\ At(t, "a_direct")
    /\ IF back = {} THEN UNCHANGED <<back, user>>
       ELSE \E o \in back : back' = back \ {o} /\ user' = user \cup {o}
    /\ Return(t)
    /\ UNCHANGED <<slab, spFull, spEmpty, spMark, drained, depot, mag>>

\* Step 1: a non-empty magazine from the cid's spares or the depot, or an
\* empty one to fill.
AGet(t) ==
    /\ At(t, "a_get")
    /\ \/ \E c \in Cids : \E m \in spFull[c] :
            /\ ~spMark[c]
            /\ spFull' = [spFull EXCEPT ![c] = @ \ {m}]
            /\ SetTop(t, [Top(t) EXCEPT !.hm = m, !.pc = "a_ins"])
            /\ UNCHANGED <<spEmpty, depot>>
       \/ \E m \in depot :
            /\ mag[m] # {}
            /\ depot' = depot \ {m}
            /\ SetTop(t, [Top(t) EXCEPT !.hm = m, !.pc = "a_ins"])
            /\ UNCHANGED <<spFull, spEmpty>>
       \/ \E c \in Cids : \E m \in spEmpty[c] :
            /\ ~spMark[c]
            /\ spEmpty' = [spEmpty EXCEPT ![c] = @ \ {m}]
            /\ SetTop(t, [Top(t) EXCEPT !.hm = m, !.pc = "a_fill"])
            /\ UNCHANGED <<spFull, depot>>
       \/ \E m \in depot :
            /\ mag[m] = {}
            /\ depot' = depot \ {m}
            /\ SetTop(t, [Top(t) EXCEPT !.hm = m, !.pc = "a_fill"])
            /\ UNCHANGED <<spFull, spEmpty>>
       \/ /\ \A m \in depot : FALSE        \* no magazine to be had
          /\ Goto(t, "a_direct")
          /\ UNCHANGED <<spFull, spEmpty, depot>>
       \/ /\ BadCid                         \* the cid went out of range
          /\ Goto(t, "a_direct")
          /\ UNCHANGED <<spFull, spEmpty, depot>>
    /\ UNCHANGED <<slab, spMark, drained, mag, back, user>>

\* Fill the held magazine from the back end. If there is nothing, put the
\* magazine back and fail the request.
AFill(t) ==
    /\ At(t, "a_fill")
    /\ IF back = {}
       THEN /\ depot' = depot \cup {Top(t).hm}
            /\ Return(t)
            /\ UNCHANGED <<mag, back>>
       ELSE \E S \in SUBSET back \ {{}} :
            /\ Cardinality(S) <= Cap
            /\ back' = back \ S
            /\ mag' = [mag EXCEPT ![Top(t).hm] = S]
            /\ Goto(t, "a_ins")
            /\ UNCHANGED depot
    /\ UNCHANGED <<slab, spFull, spEmpty, spMark, drained, user>>

\* Step 2: the install routine. On the marker, or with no cid, serve the
\* request from the held magazine and send it to the depot.
AInstall(t) ==
    /\ At(t, "a_ins")
    /\ LET f == Top(t) IN
       \E c \in SomeCid :
        IF c = NoCid \/ slab[c] = Marker
        THEN \E o \in mag[f.hm] :
                /\ mag' = [mag EXCEPT ![f.hm] = @ \ {o}]
                /\ user' = user \cup {o}
                /\ Goto(t, "to_depot")
                /\ UNCHANGED slab
        ELSE IF slab[c] = Zero \/ mag[slab[c]] = {}
        THEN /\ slab' = [slab EXCEPT ![c] = f.hm]
             /\ SetTop(t, [f EXCEPT !.hm = slab[c], !.pc = "a_retire"])
             /\ UNCHANGED <<mag, user>>
        ELSE Goto(t, "a_back") /\ UNCHANGED <<slab, mag, user>>
    /\ UNCHANGED <<spFull, spEmpty, spMark, drained, depot, back>>

\* Put a magazine into a cid's spares if they have room and are not marked,
\* or into the depot.
PutAway(m, full) ==
    \/ /\ depot' = depot \cup {m}
       /\ UNCHANGED <<spFull, spEmpty>>
    \/ \E c \in Cids :
        /\ ~spMark[c]
        /\ IF full
           THEN /\ Cardinality(spFull[c]) < SpareMax
                /\ spFull' = [spFull EXCEPT ![c] = @ \cup {m}]
                /\ UNCHANGED <<spEmpty, depot>>
           ELSE /\ Cardinality(spEmpty[c]) < SpareMax
                /\ spEmpty' = [spEmpty EXCEPT ![c] = @ \cup {m}]
                /\ UNCHANGED <<spFull, depot>>

\* Step 3: retire the displaced magazine (a_retire), or put back the one that
\* was not installed (a_back), and retry.
APut(t) ==
    /\ At(t, "a_retire") \/ At(t, "a_back")
    /\ LET f == Top(t) IN
        /\ IF f.hm = Zero THEN UNCHANGED <<spFull, spEmpty, depot>>
           ELSE PutAway(f.hm, mag[f.hm] # {})
        /\ SetTop(t, [f EXCEPT !.hm = Zero, !.pc = "a_fast"])
    /\ UNCHANGED <<slab, spMark, drained, mag, back, user>>

ToDepot(t) ==
    /\ At(t, "to_depot")
    /\ depot' = depot \cup {Top(t).hm}
    /\ Return(t)
    /\ UNCHANGED <<slab, spFull, spEmpty, spMark, drained, mag, back, user>>

----------------------------------------------------------------------------
(* Free: the mirror image *)

FFast(t) ==
    /\ At(t, "f_fast")
    /\ \E c \in SomeCid :
        IF c = NoCid THEN Goto(t, "f_direct") /\ UNCHANGED mag
        ELSE IF IsMag(slab[c]) /\ Cardinality(mag[slab[c]]) < Cap
        THEN /\ mag' = [mag EXCEPT ![slab[c]] = @ \cup {Top(t).o}]
             /\ Return(t)
        ELSE Goto(t, "f_get") /\ UNCHANGED mag
    /\ UNCHANGED <<slab, spFull, spEmpty, spMark, drained, depot, back, user>>

FDirect(t) ==
    /\ At(t, "f_direct")
    /\ back' = back \cup {Top(t).o}
    /\ Return(t)
    /\ UNCHANGED <<slab, spFull, spEmpty, spMark, drained, depot, mag, user>>

FGet(t) ==
    /\ At(t, "f_get")
    /\ \/ \E c \in Cids : \E m \in spEmpty[c] :
            /\ ~spMark[c]
            /\ spEmpty' = [spEmpty EXCEPT ![c] = @ \ {m}]
            /\ SetTop(t, [Top(t) EXCEPT !.hm = m, !.pc = "f_ins"])
            /\ UNCHANGED depot
       \/ \E m \in depot :
            /\ mag[m] = {}
            /\ depot' = depot \ {m}
            /\ SetTop(t, [Top(t) EXCEPT !.hm = m, !.pc = "f_ins"])
            /\ UNCHANGED spEmpty
       \/ /\ \A m \in depot : mag[m] # {}   \* no empty magazine to be had
          /\ Goto(t, "f_direct")
          /\ UNCHANGED <<spEmpty, depot>>
       \/ /\ BadCid
          /\ Goto(t, "f_direct")
          /\ UNCHANGED <<spEmpty, depot>>
    /\ UNCHANGED <<slab, spFull, spMark, drained, mag, back, user>>

FInstall(t) ==
    /\ At(t, "f_ins")
    /\ LET f == Top(t) IN
       \E c \in SomeCid :
        IF c = NoCid \/ slab[c] = Marker
        THEN /\ mag' = [mag EXCEPT ![f.hm] = @ \cup {f.o}]
             /\ SetTop(t, [f EXCEPT !.o = NoObj, !.pc = "to_depot"])
             /\ UNCHANGED slab
        ELSE IF slab[c] = Zero \/ Cardinality(mag[slab[c]]) = Cap
        THEN /\ slab' = [slab EXCEPT ![c] = f.hm]
             /\ SetTop(t, [f EXCEPT !.hm = slab[c], !.pc = "f_retire"])
             /\ UNCHANGED mag
        ELSE Goto(t, "f_back") /\ UNCHANGED <<slab, mag>>
    /\ UNCHANGED <<spFull, spEmpty, spMark, drained, depot, back, user>>

FPut(t) ==
    /\ At(t, "f_retire") \/ At(t, "f_back")
    /\ LET f == Top(t) IN
        /\ IF f.hm = Zero THEN UNCHANGED <<spFull, spEmpty, depot>>
           ELSE PutAway(f.hm, mag[f.hm] # {})
        /\ SetTop(t, [f EXCEPT !.hm = Zero, !.pc = "f_fast"])
    /\ UNCHANGED <<slab, spMark, drained, mag, back, user>>

\* Launder the full magazine the install displaced instead of keeping it.
FLaunder(t) ==
    /\ At(t, "f_retire")
    /\ Top(t).hm # Zero
    /\ back' = back \cup mag[Top(t).hm]
    /\ mag' = [mag EXCEPT ![Top(t).hm] = {}]
    /\ UNCHANGED <<slab, spFull, spEmpty, spMark, drained, depot, user, stack>>

----------------------------------------------------------------------------
(* Centring: the swap that leaves its magazine half full *)

\* The take: the store-if-equal routine replaces an empty loaded magazine (for
\* an allocation) or a full one (for a free) with Zero, and the call holds it.
\* Where it would not match, the call goes on as a plain install does.
CentreTake(t) ==
    /\ At(t, "a_ins") \/ At(t, "f_ins")
    /\ LET f == Top(t) IN
       \E c \in Cids :
        /\ IsMag(slab[c])
        /\ IF f.pc = "a_ins"
           THEN mag[slab[c]] = {} /\ Cardinality(mag[f.hm]) >= 2
           ELSE Cardinality(mag[slab[c]]) = Cap /\ Cap >= 2
        /\ slab' = [slab EXCEPT ![c] = Zero]
        /\ SetTop(t, [f EXCEPT !.om = slab[c],
                               !.pc = IF f.pc = "a_ins" THEN "a_copy" ELSE "f_copy"])
    /\ UNCHANGED <<spFull, spEmpty, spMark, drained, depot, mag, back, user>>

\* The copy: part of the objects of the magazine that has them goes to the
\* other. Both are the call's own, so this is no commit.
CentreCopy(t) ==
    /\ At(t, "a_copy") \/ At(t, "f_copy")
    /\ LET f == Top(t)
           from == IF f.pc = "a_copy" THEN f.hm ELSE f.om
           into == IF f.pc = "a_copy" THEN f.om ELSE f.hm IN
       \E S \in SUBSET mag[from] :
        /\ S # {} /\ S # mag[from]
        /\ mag' = [mag EXCEPT ![from] = @ \ S, ![into] = S]
        /\ Goto(t, IF f.pc = "a_copy" THEN "a_cins" ELSE "f_cins")
    /\ UNCHANGED <<slab, spFull, spEmpty, spMark, drained, depot, back, user>>

\* The install, expecting Zero. On success the magazine taken out is retired
\* as a displaced one is, and the call retries on its fast path. On the
\* marker both magazines are put away and the call retries. On another
\* magazine, which a call that ran meanwhile installed, or with no cid, the
\* call is served from the one it meant to install, which goes to the depot,
\* and the one taken out is put away.
CentreInstall(t) ==
    /\ At(t, "a_cins") \/ At(t, "f_cins")
    /\ LET f == Top(t)
           alloc == f.pc = "a_cins" IN
       \E c \in SomeCid :
        IF c # NoCid /\ (slab[c] = Zero \/ BlindCentre)
        THEN /\ slab' = [slab EXCEPT ![c] = f.hm]
             /\ SetTop(t, [f EXCEPT !.hm = f.om, !.om = Zero,
                                    !.pc = IF alloc THEN "a_retire" ELSE "f_retire"])
             /\ UNCHANGED <<mag, user>>
        ELSE IF c # NoCid /\ slab[c] = Marker
        THEN /\ Goto(t, IF alloc THEN "a_cback" ELSE "f_cback")
             /\ UNCHANGED <<slab, mag, user>>
        ELSE IF alloc
        THEN \E o \in mag[f.hm] :
                /\ mag' = [mag EXCEPT ![f.hm] = @ \ {o}]
                /\ user' = user \cup {o}
                /\ Goto(t, "c_serve")
                /\ UNCHANGED slab
        ELSE /\ mag' = [mag EXCEPT ![f.hm] = @ \cup {f.o}]
             /\ SetTop(t, [f EXCEPT !.o = NoObj, !.pc = "c_serve"])
             /\ UNCHANGED <<slab, user>>
    /\ UNCHANGED <<spFull, spEmpty, spMark, drained, depot, back>>

\* Put away the magazine a failed centring took out, then go on with the one
\* it meant to install: put it away and retry (a_back, f_back), or send it to
\* the depot (to_depot).
CentrePut(t) ==
    /\ At(t, "a_cback") \/ At(t, "f_cback") \/ At(t, "c_serve")
    /\ LET f == Top(t) IN
        /\ PutAway(f.om, mag[f.om] # {})
        /\ SetTop(t, [f EXCEPT !.om = Zero,
                               !.pc = CASE f.pc = "a_cback" -> "a_back"
                                        [] f.pc = "f_cback" -> "f_back"
                                        [] OTHER -> "to_depot"])
    /\ UNCHANGED <<slab, spMark, drained, mag, back, user>>

----------------------------------------------------------------------------
(* The drainer, as an environment. Drain.tla models it in full. *)

DrainMark(c) ==
    /\ slab[c] # Marker
    /\ drained' = [drained EXCEPT ![c] =
                    (IF IsMag(slab[c]) THEN {slab[c]} ELSE {}) \cup spFull[c] \cup spEmpty[c]]
    /\ slab' = [slab EXCEPT ![c] = Marker]
    /\ spFull' = [spFull EXCEPT ![c] = {}]
    /\ spEmpty' = [spEmpty EXCEPT ![c] = {}]
    /\ spMark' = [spMark EXCEPT ![c] = TRUE]
    /\ UNCHANGED <<depot, mag, back, user, stack>>

DrainTake(c) ==
    /\ slab[c] = Marker
    /\ depot' = depot \cup drained[c]
    /\ drained' = [drained EXCEPT ![c] = {}]
    /\ slab' = [slab EXCEPT ![c] = Zero]
    /\ spMark' = [spMark EXCEPT ![c] = FALSE]
    /\ UNCHANGED <<spFull, spEmpty, mag, back, user, stack>>

Step(t) ==
    \/ AFast(t) \/ ADirect(t) \/ AGet(t) \/ AFill(t) \/ AInstall(t) \/ APut(t) \/ ToDepot(t)
    \/ FFast(t) \/ FDirect(t) \/ FGet(t) \/ FInstall(t) \/ FPut(t) \/ FLaunder(t)
    \/ CentreTake(t) \/ CentreCopy(t) \/ CentreInstall(t) \/ CentrePut(t)

Next ==
    \/ \E t \in Threads : Call(t) \/ Step(t)
    \/ \E c \in Cids : DrainMark(c) \/ DrainTake(c)

Spec == Init /\ [][Next]_vars

Symm == Permutations(Threads) \cup Permutations(Cids) \cup Permutations(Mags) \cup Permutations(Objs)

----------------------------------------------------------------------------

Frames == {<<t, i>> : t \in Threads, i \in 1..MaxDepth}
Live(F) == {x \in F : x[2] <= Depth(x[1])}
Fr(x) == stack[x[1]][x[2]]

MagPlaces(m) ==
       {<<"slab", c>> : c \in {c \in Cids : slab[c] = m}}
  \cup {<<"spare", c>> : c \in {c \in Cids : m \in spFull[c] \cup spEmpty[c]}}
  \cup {<<"both", c>> : c \in {c \in Cids : m \in spFull[c] \cap spEmpty[c]}}
  \cup {<<"drained", c>> : c \in {c \in Cids : m \in drained[c]}}
  \cup (IF m \in depot THEN {<<"depot">>} ELSE {})
  \cup {<<"held", x>> : x \in {x \in Live(Frames) : Fr(x).hm = m}}
  \cup {<<"taken", x>> : x \in {x \in Live(Frames) : Fr(x).om = m}}

ObjPlaces(o) ==
       {<<"mag", m>> : m \in {m \in Mags : o \in mag[m]}}
  \cup (IF o \in back THEN {<<"back">>} ELSE {})
  \cup (IF o \in user THEN {<<"user">>} ELSE {})
  \cup {<<"freeing", x>> : x \in {x \in Live(Frames) : Fr(x).o = o}}

\* Rule 4: between any two steps, everything is in exactly one place: shared
\* state, or one call's private hands.
OneOwner ==
    /\ \A m \in Mags : Cardinality(MagPlaces(m)) = 1
    /\ \A o \in Objs : Cardinality(ObjPlaces(o)) = 1
    /\ \A m \in Mags : Cardinality(mag[m]) <= Cap

\* The spare stacks hold what their names say, and a magazine about to be
\* installed can serve the request if the word turns out to be the marker.
Usable ==
    /\ \A c \in Cids : \A m \in spFull[c] : mag[m] # {}
    /\ \A c \in Cids : \A m \in spEmpty[c] : mag[m] = {}
    /\ \A x \in Live(Frames) :
        /\ Fr(x).pc = "a_ins" => mag[Fr(x).hm] # {}
        /\ Fr(x).pc = "f_ins" => Cardinality(mag[Fr(x).hm]) < Cap
        /\ Fr(x).pc = "a_cins" => mag[Fr(x).hm] # {}
        /\ Fr(x).pc = "f_cins" => Cardinality(mag[Fr(x).hm]) < Cap

\* No thread ever waits: a call in progress always has a step it can take,
\* whatever the other threads and the drainer are doing.
NoWait == \A t \in Threads : Depth(t) > 0 => ENABLED Step(t)

=============================================================================
