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

   The design states these for one level. Applying them at every level is the
   reading modelled here: a summary word is "a word" for the level above.

   Reread = FALSE drops the re-read and shows why it is there.
   Heal = TRUE is a proposed change: a free walks all the way up and sets any
   summary bit it finds clear, even when its word was not empty.
   Crash = TRUE lets every thread but one vanish at an arbitrary step, once,
   which is what a forked child sees. *)
EXTENDS Naturals, FiniteSets, TLC

CONSTANTS Threads, B, Top, Reread, Heal, Crash

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
    crashed,
    gone,     \* ghost: objects that vanished threads were holding
    touched   \* ghost: level-0 words freed into since the crash

vars == <<bits, live, th, crashed, gone, touched>>

Idle == [pc |-> "idle", k |-> 0, i |-> 0, must |-> FALSE, pe |-> FALSE, set |-> {}]

Init ==
    /\ bits = [k \in Levels |-> [i \in Words(k) |-> Bits]]
    /\ live = {}
    /\ th = [t \in Threads |-> Idle]
    /\ crashed = FALSE
    /\ gone = {}
    /\ touched = {}

Set(t, T) == th' = [th EXCEPT ![t] = T]
Parent(i) == i \div B
Bit(i) == i % B
ObjsOf(i, S) == {i * B + b : b \in S}

----------------------------------------------------------------------------
(* Free *)

\* Take some live objects that share a bitmap word, as a launder does.
FreeBegin(t) ==
    /\ th[t].pc = "idle"
    /\ \E i \in Words(0) : \E S \in SUBSET Bits \ {{}} :
        /\ ObjsOf(i, S) \subseteq live
        /\ live' = live \ ObjsOf(i, S)
        /\ Set(t, [Idle EXCEPT !.pc = "f_or", !.i = i, !.set = S])
    /\ UNCHANGED <<bits, crashed, gone, touched>>

\* One atomic OR. If the word was empty the summary bit must be set.
FreeOr(t) ==
    /\ th[t].pc = "f_or"
    /\ LET T == th[t]
           old == bits[0][T.i] IN
        /\ bits' = [bits EXCEPT ![0][T.i] = old \cup T.set]
        /\ touched' = IF crashed THEN touched \cup {T.i} ELSE touched
        /\ IF old = {} \/ Heal
           THEN Set(t, [Idle EXCEPT !.pc = "up", !.i = T.i, !.must = (old = {})])
           ELSE Set(t, Idle)
    /\ UNCHANGED <<live, crashed, gone>>

\* Walk up from word (k, i). With `must`, set its summary bit with an atomic
\* OR and carry on only if the summary word was empty. Without it (Heal
\* only), read the summary bit and set it if it is clear.
Up(t) ==
    /\ th[t].pc = "up"
    /\ LET T == th[t]
           p == Parent(T.i)
           old == bits[T.k + 1][p] IN
        IF T.k = Top THEN Set(t, Idle) /\ UNCHANGED bits
        ELSE IF T.must
        THEN /\ bits' = [bits EXCEPT ![T.k + 1][p] = old \cup {Bit(T.i)}]
             /\ IF old = {} \/ Heal
                THEN Set(t, [T EXCEPT !.k = T.k + 1, !.i = p, !.must = (old = {})])
                ELSE Set(t, Idle)
        ELSE /\ UNCHANGED bits
             /\ IF Bit(T.i) \in old
                THEN Set(t, [T EXCEPT !.k = T.k + 1, !.i = p])
                ELSE Set(t, [T EXCEPT !.must = TRUE])
    /\ UNCHANGED <<live, crashed, gone, touched>>

----------------------------------------------------------------------------
(* Claim *)

SearchBegin(t) ==
    /\ th[t].pc = "idle"
    /\ Set(t, [Idle EXCEPT !.pc = "s_read", !.k = Top])
    /\ UNCHANGED <<bits, live, crashed, gone, touched>>

\* Read one word on the way down. Any set bit may be followed; the design
\* follows the lowest, which is one of the choices.
SearchRead(t) ==
    /\ th[t].pc = "s_read"
    /\ LET T == th[t]
           w == bits[T.k][T.i] IN
        IF w = {}
        THEN IF T.k = Top
             THEN Set(t, Idle)
             ELSE Set(t, [T EXCEPT !.pc = "clr"])       \* a stale hint
        ELSE IF T.k = 0
        THEN Set(t, [T EXCEPT !.pc = "s_cas", !.set = w])
        ELSE \E b \in w : Set(t, [T EXCEPT !.k = T.k - 1, !.i = T.i * B + b])
    /\ UNCHANGED <<bits, live, crashed, gone, touched>>

\* Take some of the bits read, with one compare-and-swap.
SearchCas(t) ==
    /\ th[t].pc = "s_cas"
    /\ LET T == th[t] IN
        IF bits[0][T.i] = T.set
        THEN \E S \in SUBSET T.set \ {{}} :
                /\ bits' = [bits EXCEPT ![0][T.i] = T.set \ S]
                /\ live' = live \cup ObjsOf(T.i, S)
                /\ IF S = T.set
                   THEN Set(t, [T EXCEPT !.pc = "clr", !.set = {}])
                   ELSE Set(t, Idle)
        ELSE Set(t, Idle) /\ UNCHANGED <<bits, live>>
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
    /\ UNCHANGED <<live, crashed, gone, touched>>

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
    /\ UNCHANGED <<bits, live, crashed, gone, touched>>

----------------------------------------------------------------------------

\* Fork: one thread survives. Objects the others were about to free are lost
\* with them.
CrashAllBut(t) ==
    /\ Crash /\ ~crashed
    /\ crashed' = TRUE
    /\ gone' = UNION {ObjsOf(th[u].i, th[u].set) : u \in {u \in Threads \ {t} : th[u].pc = "f_or"}}
    /\ th' = [u \in Threads |-> IF u = t THEN th[t] ELSE Idle]
    /\ UNCHANGED <<bits, live, touched>>

Next == \E t \in Threads :
    \/ FreeBegin(t) \/ FreeOr(t) \/ Up(t)
    \/ SearchBegin(t) \/ SearchRead(t) \/ SearchCas(t) \/ Clear(t) \/ ReadAgain(t)
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

\* Thread t is still going to set, or reconsider, the summary bit of (k, i).
Owes(t, k, i) ==
    /\ th[t].k = k /\ th[t].i = i
    /\ th[t].pc \in {"up", "reread"}

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
