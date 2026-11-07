# Safety ledger: `crates/cralloc/src/heap/source.rs`

## HEAP-SOURCE-01: `self.launder(class, objects)` in `take_back`

**Precondition** (`Back::launder`). Every pointer that is a handed-out slot of `class` is one
the caller owns and gives up.

**Proof.** `take_back` is the depot's way to give objects back (`depot::Source`): its
contract is that each object is a free object of `class` that this source handed out. The
depot holds only objects a program gave up (frees, through the push routine or a slow path)
or that this source filled and nobody has been given, and each in one place only: the slab
invariant (`front/real-safety.md`) and the depot's single-owner rule for a magazine it has
taken (design: "Concurrency rules", rule 4). So every object here is one the depot owns and is
giving up. A program that freed a block twice breaks that, and `launder` then aborts or, if
the copies never meet, hands the slot out twice: SLOW-03 in `c/slow-safety.md` says what that does.

Reviewed: 7ddb618