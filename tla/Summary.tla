------------------------------ MODULE Summary ------------------------------
(* Summary bits over a region's free bitmap.

   Level 0 is the free bitmap: one bit per object, B bits to a word. Each
   word of level k has one summary bit in level k+1, and the top level is a
   single word. A set summary bit means "something below may be free".

     free    set the word's bits with one atomic OR. If the word was empty,
             set its summary bit, and so on upward.
     claim   walk down from the top through set bits to a level-0 word and
             take bits from it with one compare-and-swap.
     clear   whoever empties a word, or finds an empty word under a set
             summary bit, clears the summary bit and then reads the word
             again; if it is no longer empty, sets the bit back.

   These apply at every level: a summary word is "a word" for the level above.

   Reread = FALSE drops the re-read and shows why it is there.
   Heal = TRUE is the design's free: it walks all the way up and sets any
   summary bit it finds clear, even when its word was not empty. FALSE is the
   free described above, which a fork defeats (Summary_crash).
   Crash = TRUE lets every thread but one vanish at an arbitrary step, once,
   which is what a forked child sees.
   Run = TRUE adds what a run of claims (a fill) does:
     - a search may start at any word, as a run that resumes at its cursor
       or moves past another thread does;
     - a search that finds a word empty may leave it without clearing its
       bit, as a run leaves a word another thread emptied to that thread;
     - a thread that empties a level-0 word may hold its clear, and later
       clear the bits of every word it holds under one summary word with one
       atomic AND, then read each of those words again: a word no longer
       empty has its bit set back by a walk up, and if the AND emptied the
       summary word and every word read empty, the thread clears the summary
       word's own bit, as a single clear would.
     A purge's claims (tree::ExactRun) are these same steps. Each word is a
     search that starts there and takes the bits it wants, or gives up having
     written nothing, as a thread leaving this search does. It holds its clears
     within a group and makes them before it returns any page. What it then
     holds is live here, for as long as a program could hold it: madvise
     changes no bit. Giving back, after the madvise or after meeting a word not
     wholly free, is a free of each word, and the batch that shares their walk
     (tree::Release) is those frees paused between OR and walk, the walks made
     one after another. A small class's purges are Summary_run and Summary_look;
     a large class's meet kept bits and frees that look at every level, which
     is SummaryLookUp. No model here says a fill does not wait for a purge: that
     is a matter of how far its search walks, which back/tests/purge_claim.rs
     counts and a_purge_lost_at_any_step_holds_up_no_fill explores.
   Keep = TRUE lets a thread that empties a level-0 word leave its summary
   bit set and owe nothing, under any number of summary words, as a large
   class's claim does: the bit is cleared by the next search that finds the
   word empty.
   Look = TRUE lets each free whose OR found its word empty choose to read the
   summary bit before setting it, as a free into a word that was not empty
   does (Heal), instead of setting it without looking: a launder of scattered
   slots, which mostly finds the bit set by its neighbours, then makes a plain
   load where it would make a locked OR. The choice is per free because the
   code mixes them on one tree: scattered launders look, grouped launders and
   single frees do not. *)
EXTENDS Naturals, FiniteSets, TLC

CONSTANTS Threads, B, Top, Reread, Heal, Crash, Run, Keep, Look

Bits == 0..(B - 1)

RECURSIVE Pow(_)
Pow(n) == IF n = 0 THEN 1 ELSE B * Pow(n - 1)

Levels == 0..Top
Words(k) == 0..(Pow(Top - k) - 1)
Objs == 0..(Pow(Top + 1) - 1)

VARIABLES
    bits,     \* bits[k][i]: the set bits of word i at level k
    live,     \* objects the program holds
    th,       \* per-thread state
    held,     \* held[t]: level-0 words t emptied and has not yet cleared
    crashed,
    gone,     \* ghost: objects that vanished threads were holding
    touched   \* ghost: level-0 words freed into since the crash

vars == <<bits, live, th, held, crashed, gone, touched>>

\* g: the summary word of a batch; rr: its words still to read again; ok: every
\* one read so far was empty; ret: an up walk that returns to the batch.
Idle == [pc |-> "idle", k |-> 0, i |-> 0, must |-> FALSE, pe |-> FALSE, set |-> {},
         g |-> 0, rr |-> {}, ok |-> TRUE, ret |-> FALSE]

Init ==
    /\ bits = [k \in Levels |-> [i \in Words(k) |-> Bits]]
    /\ live = {}
    /\ th = [t \in Threads |-> Idle]
    /\ held = [t \in Threads |-> {}]
    /\ crashed = FALSE
    /\ gone = {}
    /\ touched = {}

Set(t, T) == th' = [th EXCEPT ![t] = T]
Parent(i) == i \div B
Bit(i) == i % B
ObjsOf(i, S) == {i * B + b : b \in S}

\* The end of an up walk: back to the batch it was part of, or idle.
WalkDone(t, T) ==
    IF T.ret
    THEN Set(t, [T EXCEPT !.pc = "breread", !.ret = FALSE, !.must = FALSE])
    ELSE Set(t, Idle)

----------------------------------------------------------------------------
(* Free *)

\* Take some live objects that share a bitmap word, as a launder does.
FreeBegin(t) ==
    /\ th[t].pc = "idle"
    /\ \E i \in Words(0) : \E S \in SUBSET Bits \ {{}} :
        /\ ObjsOf(i, S) \subseteq live
        /\ live' = live \ ObjsOf(i, S)
        /\ Set(t, [Idle EXCEPT !.pc = "f_or", !.i = i, !.set = S])
    /\ UNCHANGED <<bits, held, crashed, gone, touched>>

\* One atomic OR. If the word was empty the summary bit must be set.
FreeOr(t) ==
    /\ th[t].pc = "f_or"
    /\ LET T == th[t]
           old == bits[0][T.i] IN
        /\ bits' = [bits EXCEPT ![0][T.i] = old \cup T.set]
        /\ touched' = IF crashed THEN touched \cup {T.i} ELSE touched
        /\ IF old = {} \/ Heal
           THEN \E look \in IF Look THEN {TRUE, FALSE} ELSE {FALSE} :
                    Set(t, [Idle EXCEPT !.pc = "up", !.i = T.i, !.must = (old = {} /\ ~look)])
           ELSE Set(t, Idle)
    /\ UNCHANGED <<live, held, crashed, gone>>

\* Walk up from word (k, i). With `must`, set its summary bit with an atomic
\* OR and carry on only if the summary word was empty. Without it (Heal
\* only), read the summary bit and set it if it is clear.
Up(t) ==
    /\ th[t].pc = "up"
    /\ LET T == th[t]
           p == Parent(T.i)
           old == bits[T.k + 1][p] IN
        IF T.k = Top THEN WalkDone(t, T) /\ UNCHANGED bits
        ELSE IF T.must
        THEN /\ bits' = [bits EXCEPT ![T.k + 1][p] = old \cup {Bit(T.i)}]
             /\ IF old = {} \/ Heal
                THEN Set(t, [T EXCEPT !.k = T.k + 1, !.i = p, !.must = (old = {})])
                ELSE WalkDone(t, T)
        ELSE /\ UNCHANGED bits
             /\ IF Bit(T.i) \in old
                THEN Set(t, [T EXCEPT !.k = T.k + 1, !.i = p])
                ELSE Set(t, [T EXCEPT !.must = TRUE])
    /\ UNCHANGED <<live, held, crashed, gone, touched>>

----------------------------------------------------------------------------
(* Claim *)

\* A search starts at the top word, or with Run at any word.
SearchBegin(t) ==
    /\ th[t].pc = "idle"
    /\ \/ Set(t, [Idle EXCEPT !.pc = "s_read", !.k = Top])
       \/ Run /\ \E k \in Levels : \E i \in Words(k) :
                    Set(t, [Idle EXCEPT !.pc = "s_read", !.k = k, !.i = i])
    /\ UNCHANGED <<bits, live, held, crashed, gone, touched>>

\* Read one word on the way down. Any set bit may be followed; the design
\* follows the lowest, which is one of the choices.
SearchRead(t) ==
    /\ th[t].pc = "s_read"
    /\ LET T == th[t]
           w == bits[T.k][T.i] IN
        IF w = {}
        THEN IF T.k = Top
             THEN Set(t, Idle)
             ELSE \/ Set(t, [T EXCEPT !.pc = "clr"])       \* a stale hint
                  \/ Run /\ Set(t, Idle)                   \* left to another
        ELSE IF T.k = 0
        THEN Set(t, [T EXCEPT !.pc = "s_cas", !.set = w])
        ELSE \E b \in w : Set(t, [T EXCEPT !.k = T.k - 1, !.i = T.i * B + b])
    /\ UNCHANGED <<bits, live, held, crashed, gone, touched>>

\* Take some of the bits read, with one compare-and-swap. A thread that empties
\* the word clears its bit now or, with Keep, never; or, with Run, holds the
\* clear: in a batch of words under one summary word.
SearchCas(t) ==
    /\ th[t].pc = "s_cas"
    /\ LET T == th[t] IN
        IF bits[0][T.i] = T.set
        THEN \E S \in SUBSET T.set \ {{}} :
                /\ bits' = [bits EXCEPT ![0][T.i] = T.set \ S]
                /\ live' = live \cup ObjsOf(T.i, S)
                /\ IF S = T.set
                   THEN \/ Set(t, [T EXCEPT !.pc = "clr", !.set = {}]) /\ UNCHANGED held
                        \/ Keep /\ Set(t, Idle) /\ UNCHANGED held
                        \/ /\ Run /\ Top > 0
                           /\ \A j \in held[t] : Parent(j) = Parent(T.i)
                           /\ held' = [held EXCEPT ![t] = @ \cup {T.i}]
                           /\ Set(t, Idle)
                   ELSE Set(t, Idle) /\ UNCHANGED held
        ELSE Set(t, Idle) /\ UNCHANGED <<bits, live, held>>
    /\ UNCHANGED <<crashed, gone, touched>>

\* Word (k, i) is empty: clear its summary bit with an atomic AND.
Clear(t) ==
    /\ th[t].pc = "clr"
    /\ LET T == th[t]
           p == Parent(T.i)
           new == bits[T.k + 1][p] \ {Bit(T.i)} IN
        IF T.k = Top THEN Set(t, Idle) /\ UNCHANGED bits
        ELSE /\ bits' = [bits EXCEPT ![T.k + 1][p] = new]
             /\ IF Reread
                THEN Set(t, [T EXCEPT !.pc = "reread", !.pe = (new = {})])
                ELSE IF new = {}
                THEN Set(t, [T EXCEPT !.k = T.k + 1, !.i = p])
                ELSE Set(t, Idle)
    /\ UNCHANGED <<live, held, crashed, gone, touched>>

\* Read the word again. If something was freed into it meanwhile, put the
\* summary bit back. Otherwise, if clearing the bit emptied the summary
\* word, that word is now one we emptied: go up.
ReadAgain(t) ==
    /\ th[t].pc = "reread"
    /\ LET T == th[t] IN
        IF bits[T.k][T.i] # {}
        THEN Set(t, [T EXCEPT !.pc = "up", !.must = TRUE, !.pe = FALSE])
        ELSE IF T.pe
        THEN Set(t, [T EXCEPT !.pc = "clr", !.k = T.k + 1, !.i = Parent(T.i), !.pe = FALSE])
        ELSE Set(t, Idle)
    /\ UNCHANGED <<bits, live, held, crashed, gone, touched>>

\* The held clears, made with one atomic AND of their summary word.
ClearHeld(t) ==
    /\ th[t].pc = "idle" /\ held[t] # {}
    /\ LET g == Parent(CHOOSE j \in held[t] : TRUE)
           new == bits[1][g] \ {Bit(j) : j \in held[t]} IN
        /\ bits' = [bits EXCEPT ![1][g] = new]
        /\ held' = [held EXCEPT ![t] = {}]
        /\ IF Reread
           THEN Set(t, [Idle EXCEPT !.pc = "breread", !.g = g, !.rr = held[t],
                                    !.pe = (new = {}), !.ok = TRUE])
           ELSE IF new = {}
           THEN Set(t, [Idle EXCEPT !.pc = "clr", !.k = 1, !.i = g])
           ELSE Set(t, Idle)
    /\ UNCHANGED <<live, crashed, gone, touched>>

\* Read each word of the batch again, setting back the bit of any that is
\* not empty. When none is left: if the AND emptied the summary word and
\* every word read empty, the summary word is one this thread emptied.
BatchReread(t) ==
    /\ th[t].pc = "breread"
    /\ LET T == th[t] IN
        IF T.rr = {}
        THEN IF T.pe /\ T.ok
             THEN Set(t, [Idle EXCEPT !.pc = "clr", !.k = 1, !.i = T.g])
             ELSE Set(t, Idle)
        ELSE \E j \in T.rr :
                IF bits[0][j] # {}
                THEN Set(t, [T EXCEPT !.pc = "up", !.k = 0, !.i = j, !.must = TRUE,
                                      !.rr = T.rr \ {j}, !.ok = FALSE, !.ret = TRUE])
                ELSE Set(t, [T EXCEPT !.rr = T.rr \ {j}])
    /\ UNCHANGED <<bits, live, held, crashed, gone, touched>>

----------------------------------------------------------------------------

\* Fork: one thread survives. Objects the others were about to free are lost
\* with them, and so are the clears they held.
CrashAllBut(t) ==
    /\ Crash /\ ~crashed
    /\ crashed' = TRUE
    /\ gone' = UNION {ObjsOf(th[u].i, th[u].set) : u \in {u \in Threads \ {t} : th[u].pc = "f_or"}}
    /\ th' = [u \in Threads |-> IF u = t THEN th[t] ELSE Idle]
    /\ held' = [u \in Threads |-> IF u = t THEN held[t] ELSE {}]
    /\ UNCHANGED <<bits, live, touched>>

Next == \E t \in Threads :
    \/ FreeBegin(t) \/ FreeOr(t) \/ Up(t)
    \/ SearchBegin(t) \/ SearchRead(t) \/ SearchCas(t) \/ Clear(t) \/ ReadAgain(t)
    \/ ClearHeld(t) \/ BatchReread(t)
    \/ CrashAllBut(t)

Spec == Init /\ [][Next]_vars

Symm == Permutations(Threads)

----------------------------------------------------------------------------

FreeObjs == UNION {ObjsOf(i, bits[0][i]) : i \in Words(0)}
Batch(t) == IF th[t].pc = "f_or" THEN ObjsOf(th[t].i, th[t].set) ELSE {}
Batches == UNION {Batch(t) : t \in Threads}

\* No object is handed out twice, and none is lost.
Partition ==
    /\ FreeObjs \cap live = {}
    /\ FreeObjs \cap Batches = {}
    /\ live \cap Batches = {}
    /\ \A t, u \in Threads : t # u => Batch(t) \cap Batch(u) = {}
    /\ FreeObjs \cup live \cup Batches \cup gone = Objs

\* Thread t is still going to set, or reconsider, the summary bit of (k, i):
\* the word its walk or clear is at, or a word of its batch not yet read again.
Owes(t, k, i) ==
    \/ /\ th[t].k = k /\ th[t].i = i
       /\ th[t].pc \in {"up", "reread"}
    \/ k = 0 /\ i \in th[t].rr

\* A non-empty word has its summary bit set, or some thread is about to set
\* it. So once the threads at work finish, every free object can be found
\* from the top: none is permanently invisible.
Visible ==
    \A k \in 0..(Top - 1) : \A i \in Words(k) :
        bits[k][i] # {} =>
            \/ Bit(i) \in bits[k + 1][Parent(i)]
            \/ \E t \in Threads : Owes(t, k, i)

RECURSIVE PathSet(_, _)
PathSet(k, i) ==
    k = Top \/ (Bit(i) \in bits[k + 1][Parent(i)] /\ PathSet(k + 1, Parent(i)))

Quiet == \A t \in Threads : th[t].pc = "idle"

\* After a crash: a word that has been freed into since can be found.
Healed ==
    Quiet => \A i \in touched : bits[0][i] # {} => PathSet(0, i)

=============================================================================
