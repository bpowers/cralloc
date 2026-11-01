# Safety ledger: `crates/cralloc/src/depot/spare.rs`

## SPARE-01: `stack_push` in `spare_push`

**Precondition** (`Routines::stack_push`). `magazine` is the slot-0 address of a magazine of the
size of the field's class, in committed magazine memory, that the caller holds.

**Proof.** The argument is `magazine.base()` of the `Magazine` handle `spare_push` owns (it
takes the handle by value). Its callers, `retire`, pass handles of `class.magazine()` size:
magazines obtained for `class` (DEPOT-SLOW-01) or displaced from `class`'s hot word, which
`Area::loaded` built with `class.magazine()`. `take_spares_keeping` (`age.rs`) calls it too,
for the magazines it keeps of the chain it took from `class`'s own full stack (AGE-02), each
turned into a handle with `class.magazine()` and pushed back on the full stack of the same class. Handles lie in committed magazine memory
(MAGAZINE-01). On `Pushed` the stack holds the magazine and the handle is dropped unused; on
`Marker` or `NoCpu` nothing was written and the handle comes back to the caller.

Reviewed: 3b5a3f5
