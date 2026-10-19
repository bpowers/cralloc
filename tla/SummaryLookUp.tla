---------------------------- MODULE SummaryLookUp ----------------------------
(* Summary.tla with a free that may look at every level of its walk up.

   In Summary.tla a walk up that set a summary bit with an OR and found the
   summary word empty sets the next level's bit without reading it (`must`),
   and `Look` only lets the free choose at the level above its leaf word.
   `tree::release_one` reads each level's bit before setting it, the level
   above a summary word its own OR found empty included, on a tree whose
   claims keep their bits (`Keep`) and whose purges and batched frees do not
   look. Here every step of a walk that would carry `must` upward may drop
   it instead, so the walks of both kinds, and any mix of them, are
   behaviours of this specification. *)
EXTENDS Summary

UpAny(t) ==
    /\ th[t].pc = "up"
    /\ LET T == th[t]
           p == Parent(T.i)
           old == bits[T.k + 1][p] IN
        IF T.k = Top THEN WalkDone(t, T) /\ UNCHANGED bits
        ELSE IF T.must
        THEN /\ bits' = [bits EXCEPT ![T.k + 1][p] = old \cup {Bit(T.i)}]
             /\ IF old = {} \/ Heal
                THEN \E look \in {TRUE, FALSE} :
                        Set(t, [T EXCEPT !.k = T.k + 1, !.i = p,
                                         !.must = (old = {} /\ ~look)])
                ELSE WalkDone(t, T)
        ELSE /\ UNCHANGED bits
             /\ IF Bit(T.i) \in old
                THEN Set(t, [T EXCEPT !.k = T.k + 1, !.i = p])
                ELSE Set(t, [T EXCEPT !.must = TRUE])
    /\ UNCHANGED <<live, held, crashed, gone, touched>>

NextAny == \E t \in Threads :
    \/ FreeBegin(t) \/ FreeOr(t) \/ UpAny(t)
    \/ SearchBegin(t) \/ SearchRead(t) \/ SearchCas(t) \/ Clear(t) \/ ReadAgain(t)
    \/ ClearHeld(t) \/ BatchReread(t)
    \/ CrashAllBut(t)

SpecAny == Init /\ [][NextAny]_vars

=============================================================================
