------------------------------- MODULE Assign -------------------------------
(* How a class takes a region, and how a walk that finds regions by scanning
   learns a region's class (crates/cralloc/src/back/class.rs, `Back::assign`;
   back/tags.rs).

   A region's tag in the table is changed only by compare-and-swap. A thread
   that wants a fresh region for class c reads c's carving word, tags an
   unassigned region c, and then tries to swing the carving word from what it
   read to that region. If the swing wins, the region is c's for good: no path
   takes a class's region back. If it loses (another thread named a region
   for c first), the thread puts the tag back to unassigned, and the region
   can then go to another class or to a run.

   The thread whose swing won then writes the region's class in a second
   byte, the published tag, which is written once and never changes. A walk
   (the purge's, the hold's, the tail's, the inspection) takes a region's class
   from the published tag; everything that holds a pointer into the region
   reads the table, which is right for it because a pointer exists only once
   the region was named.

   Property:
     Safe  a scanner acts on (region, class) only while the region is that
           class's, not another class's or a run's.
     Published, Kept  a published tag is the region's class for good.
   Constants:
     Assigners  the threads assigning: all for c1 (two, so that one can lose)
                but Second, for c2.
     ReadsTable TRUE: the scanner takes the class from the table, as the code
                did before the published tag; it acts on a region a loser
                tagged, and Safe fails (Assign_table).
     Repair     how a fork child's repair (`Back::publish_named`) decides to
                publish a region the table gives a class and that is not
                published. It runs beside the child's own threads, which may be
                assigning.
                "none": there is no repair.
                "carved": as the code at f07640f: it reads the tag, then the
                class's carving word, then whether the region has been carved
                from, through the host of the class it read; a carve by
                another class that took the region meanwhile reads as this
                class's, and Published fails (Assign_repair).
                "recheck": after it finds the region carved from, it reads the
                tag again and publishes only if it is still the class's
                (Assign_repair_recheck).
   The OS stops threads anywhere: every step is atomic and any interleaving is
   explored. Each assigner and the run go once; the scanner scans for ever. *)
EXTENDS Naturals, FiniteSets, TLC

CONSTANTS Regions, Assigners, Second, ReadsTable, Repair

\* Every assigner works for class c1 but `Second`, which works for c2.
ClassOf == [t \in Assigners |-> IF t = Second THEN "c2" ELSE "c1"]

Classes == {ClassOf[t] : t \in Assigners}
None == "none"

VARIABLES
    table,     \* each region's tag: "U", a class, or "run"
    published, \* each region's published tag: None or a class
    carving,   \* each class's carving word: None or a region
    named,     \* ghost: the class whose carving word named the region, or None
    pc,        \* each assigner's step, and the region it tagged and what it read
    run,       \* the run's step: "idle", "held" (one region), "done"
    runAt,
    scan,      \* the scanner: what it read, if anything
    carved,    \* each region: whether a carve has taken slots from it. One fact a region,
               \* whatever the class: two small classes' grow words are the same word, and a
               \* large class's is where a small class's top summary word is
    rep        \* the repair: its step, the region and the class it read

vars == <<table, published, carving, named, pc, run, runAt, scan, carved, rep>>

TypeOK ==
    /\ table \in [Regions -> {"U", "run"} \cup Classes]
    /\ published \in [Regions -> {None} \cup Classes]
    /\ carving \in [Classes -> {None} \cup Regions]

Init ==
    /\ table = [r \in Regions |-> "U"]
    /\ published = [r \in Regions |-> None]
    /\ carving = [c \in Classes |-> None]
    /\ named = [r \in Regions |-> None]
    /\ pc = [t \in Assigners |-> [at |-> "read", r |-> None, seen |-> None]]
    /\ run = "idle"
    /\ runAt = None
    /\ scan = [at |-> "idle", r |-> None, c |-> None]
    /\ carved = [r \in Regions |-> FALSE]
    /\ rep = [at |-> "idle", r |-> None, c |-> None]

Others == <<run, runAt, scan, carved, rep>>

\* The assigner reads its class's carving word.
Read(t) ==
    /\ pc[t].at = "read"
    /\ pc' = [pc EXCEPT ![t] = [at |-> "tag", r |-> None, seen |-> carving[ClassOf[t]]]]
    /\ UNCHANGED <<table, published, carving, named, run, runAt, scan, carved, rep>>

\* It tags an unassigned region with its class (CAS from unassigned), or stops if the carving
\* word has changed since it read it.
Tag(t, r) ==
    /\ pc[t].at = "tag"
    /\ carving[ClassOf[t]] = pc[t].seen
    /\ table[r] = "U"
    /\ table' = [table EXCEPT ![r] = ClassOf[t]]
    /\ pc' = [pc EXCEPT ![t].at = "swing", ![t].r = r]
    /\ UNCHANGED <<published, carving, named, run, runAt, scan, carved, rep>>

Quit(t) ==
    /\ pc[t].at = "tag"
    /\ carving[ClassOf[t]] # pc[t].seen
    /\ pc' = [pc EXCEPT ![t].at = "done"]
    /\ UNCHANGED <<table, published, carving, named, run, runAt, scan, carved, rep>>

\* The swing of the carving word: won, the region is named; lost, the tag goes back.
Swing(t) ==
    /\ pc[t].at = "swing"
    /\ LET c == ClassOf[t] IN
       IF carving[c] = pc[t].seen
       THEN /\ carving' = [carving EXCEPT ![c] = pc[t].r]
            /\ named' = [named EXCEPT ![pc[t].r] = c]
            /\ pc' = [pc EXCEPT ![t].at = "publish"]
            /\ UNCHANGED table
       ELSE /\ table' = [table EXCEPT ![pc[t].r] = "U"]
            /\ pc' = [pc EXCEPT ![t].at = "done"]
            /\ UNCHANGED <<carving, named>>
    /\ UNCHANGED <<published, run, runAt, scan, carved, rep>>

\* A compare-and-swap from unpublished, as `Tags::publish` is.
Publish(t) ==
    /\ pc[t].at = "publish"
    /\ published' = IF published[pc[t].r] = None
                    THEN [published EXCEPT ![pc[t].r] = ClassOf[t]]
                    ELSE published
    /\ pc' = [pc EXCEPT ![t].at = "done"]
    /\ UNCHANGED <<table, carving, named, run, runAt, scan, carved, rep>>

\* A run takes an unassigned region and later frees it.
RunTake(r) ==
    /\ run = "idle" /\ table[r] = "U"
    /\ table' = [table EXCEPT ![r] = "run"]
    /\ run' = "held" /\ runAt' = r
    /\ UNCHANGED <<published, carving, named, pc, scan, carved, rep>>

RunFree ==
    /\ run = "held"
    /\ table' = [table EXCEPT ![runAt] = "U"]
    /\ run' = "done" /\ runAt' = None
    /\ UNCHANGED <<published, carving, named, pc, scan, carved, rep>>

\* The scanner reads a region's class, and acts on it later.
ScanRead(r) ==
    /\ scan.at = "idle"
    /\ LET c == IF ReadsTable
                THEN (IF table[r] \in Classes THEN table[r] ELSE None)
                ELSE published[r]
       IN /\ c # None
          /\ scan' = [at |-> "act", r |-> r, c |-> c]
    /\ UNCHANGED <<table, published, carving, named, pc, run, runAt, carved, rep>>

ScanAct ==
    /\ scan.at = "act"
    /\ scan' = [at |-> "idle", r |-> None, c |-> None]
    /\ UNCHANGED <<table, published, carving, named, pc, run, runAt, carved, rep>>

\* Any thread of a class carves from the region its carving word names.
Carve(c) ==
    /\ carving[c] # None
    /\ ~carved[carving[c]]
    /\ carved' = [carved EXCEPT ![carving[c]] = TRUE]
    /\ UNCHANGED <<table, published, carving, named, pc, run, runAt, scan, rep>>

\* The repair, a load a step: the tag of an unpublished region, the class's carving word,
\* whether the region has been carved from, the tag again ("recheck"), and the publication.
RepStep(next) ==
    /\ rep' = next
    /\ UNCHANGED <<table, published, carving, named, pc, run, runAt, scan, carved>>

RepIdle == [at |-> "idle", r |-> None, c |-> None]

RepairTag(r) ==
    /\ Repair # "none" /\ rep.at = "idle"
    /\ published[r] = None /\ table[r] \in Classes
    /\ RepStep([at |-> "carving", r |-> r, c |-> table[r]])

RepairCarving ==
    /\ rep.at = "carving"
    /\ RepStep([rep EXCEPT !.at = IF carving[rep.c] = rep.r THEN "publish" ELSE "carved"])

RepairCarved ==
    /\ rep.at = "carved"
    /\ RepStep(IF ~carved[rep.r] THEN RepIdle
               ELSE [rep EXCEPT !.at = IF Repair = "recheck" THEN "again" ELSE "publish"])

RepairAgain ==
    /\ rep.at = "again"
    /\ RepStep(IF table[rep.r] = rep.c THEN [rep EXCEPT !.at = "publish"] ELSE RepIdle)

RepairPublish ==
    /\ rep.at = "publish"
    /\ published' = IF published[rep.r] = None
                    THEN [published EXCEPT ![rep.r] = rep.c]
                    ELSE published
    /\ rep' = RepIdle
    /\ UNCHANGED <<table, carving, named, pc, run, runAt, scan, carved>>

Next ==
    \/ \E c \in Classes : Carve(c)
    \/ \E r \in Regions : RepairTag(r)
    \/ RepairCarving \/ RepairCarved \/ RepairAgain \/ RepairPublish
    \/ \E t \in Assigners : Read(t) \/ Quit(t) \/ Swing(t) \/ Publish(t)
    \/ \E t \in Assigners, r \in Regions : Tag(t, r)
    \/ \E r \in Regions : RunTake(r) \/ ScanRead(r)
    \/ RunFree
    \/ ScanAct

Spec == Init /\ [][Next]_vars

\* The scanner about to act acts on a region that is that class's: the table holds the class,
\* so no other class and no run has it.
Safe == scan.at = "act" => table[scan.r] = scan.c

\* The weaker statement, which is the harm: the scanner never acts on a region that another
\* class or a run has. A region given back and still unassigned is all zero to every walk.
\* `Assign_table` is checked against this one, so that its counterexample is a region another
\* owner took and not merely one given back.
NotAnothers == scan.at = "act" => table[scan.r] \in {scan.c, "U"}

\* A published tag is the region's class for good: the table says the same, and it was named.
Published == \A r \in Regions :
    published[r] # None => (table[r] = published[r] /\ named[r] = published[r])

\* A class's region never goes back: once named, the table keeps the tag.
Kept == \A r \in Regions : named[r] # None => table[r] = named[r]
=============================================================================
