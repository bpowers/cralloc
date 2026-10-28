------------------------------- MODULE Assign -------------------------------
(* How a class takes a region, and how a walk that finds regions by scanning
   learns a region's class (crates/asmalloc/src/back/class.rs, `Back::assign`;
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
   The OS stops threads anywhere: every step is atomic and any interleaving is
   explored. Each assigner and the run go once; the scanner scans for ever. *)
EXTENDS Naturals, FiniteSets, TLC

CONSTANTS Regions, Assigners, Second, ReadsTable

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
    scan       \* the scanner: what it read, if anything

vars == <<table, published, carving, named, pc, run, runAt, scan>>

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

Others == <<run, runAt, scan>>

\* The assigner reads its class's carving word.
Read(t) ==
    /\ pc[t].at = "read"
    /\ pc' = [pc EXCEPT ![t] = [at |-> "tag", r |-> None, seen |-> carving[ClassOf[t]]]]
    /\ UNCHANGED <<table, published, carving, named, run, runAt, scan>>

\* It tags an unassigned region with its class (CAS from unassigned), or stops if the carving
\* word has changed since it read it.
Tag(t, r) ==
    /\ pc[t].at = "tag"
    /\ carving[ClassOf[t]] = pc[t].seen
    /\ table[r] = "U"
    /\ table' = [table EXCEPT ![r] = ClassOf[t]]
    /\ pc' = [pc EXCEPT ![t].at = "swing", ![t].r = r]
    /\ UNCHANGED <<published, carving, named, run, runAt, scan>>

Quit(t) ==
    /\ pc[t].at = "tag"
    /\ carving[ClassOf[t]] # pc[t].seen
    /\ pc' = [pc EXCEPT ![t].at = "done"]
    /\ UNCHANGED <<table, published, carving, named, run, runAt, scan>>

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
    /\ UNCHANGED <<published, run, runAt, scan>>

Publish(t) ==
    /\ pc[t].at = "publish"
    /\ published' = [published EXCEPT ![pc[t].r] = ClassOf[t]]
    /\ pc' = [pc EXCEPT ![t].at = "done"]
    /\ UNCHANGED <<table, carving, named, run, runAt, scan>>

\* A run takes an unassigned region and later frees it.
RunTake(r) ==
    /\ run = "idle" /\ table[r] = "U"
    /\ table' = [table EXCEPT ![r] = "run"]
    /\ run' = "held" /\ runAt' = r
    /\ UNCHANGED <<published, carving, named, pc, scan>>

RunFree ==
    /\ run = "held"
    /\ table' = [table EXCEPT ![runAt] = "U"]
    /\ run' = "done" /\ runAt' = None
    /\ UNCHANGED <<published, carving, named, pc, scan>>

\* The scanner reads a region's class, and acts on it later.
ScanRead(r) ==
    /\ scan.at = "idle"
    /\ LET c == IF ReadsTable
                THEN (IF table[r] \in Classes THEN table[r] ELSE None)
                ELSE published[r]
       IN /\ c # None
          /\ scan' = [at |-> "act", r |-> r, c |-> c]
    /\ UNCHANGED <<table, published, carving, named, pc, run, runAt>>

ScanAct ==
    /\ scan.at = "act"
    /\ scan' = [at |-> "idle", r |-> None, c |-> None]
    /\ UNCHANGED <<table, published, carving, named, pc, run, runAt>>

Next ==
    \/ \E t \in Assigners : Read(t) \/ Quit(t) \/ Swing(t) \/ Publish(t)
    \/ \E t \in Assigners, r \in Regions : Tag(t, r)
    \/ \E r \in Regions : RunTake(r) \/ ScanRead(r)
    \/ RunFree
    \/ ScanAct

Spec == Init /\ [][Next]_vars

\* The scanner about to act acts on a region that is that class's: the table holds the class,
\* so no other class and no run has it.
Safe == scan.at = "act" => table[scan.r] = scan.c

\* A published tag is the region's class for good: the table says the same, and it was named.
Published == \A r \in Regions :
    published[r] # None => (table[r] = published[r] /\ named[r] = published[r])

\* A class's region never goes back: once named, the table keeps the tag.
Kept == \A r \in Regions : named[r] # None => table[r] = named[r]
=============================================================================
