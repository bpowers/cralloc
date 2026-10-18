# Safety ledger: `crates/asmalloc/src/depot/slow.rs`

## DEPOT-SLOW-01: `store_if_equal` in `exchange_for_alloc` and `exchange_for_free`

**Precondition** (`Routines::store_if_equal`). `new` keeps the slab invariant for the hot word
of `class`: the address of slot `s`, `1 <= s <= n - 1`, of a magazine of the class's size in
committed magazine memory that the caller holds, with slots `1..s` holding its objects (or zero,
or a marker, which these calls never pass).

**Proof.** `new` is `held.word(level)` for a `Magazine` handle `held` this call holds:

- `held` came from `obtain_full` or `obtain_empty`, which return handles made by `Area` for
  magazines of `class.magazine()`, the class's size: from the class's spare stacks, the class's
  depot stack, the empty stack of that size, or `Area::create(class.magazine())`. Every handle
  lies in committed magazine memory (MAGAZINE-01). The thread holds it: a spare pop, a depot pop
  and a creation each give one thread the magazine, and nothing refers to it until it is
  installed, pushed or sent to the depot.
- For allocation, `level = held.level()`, which `Magazine::record` caps at `capacity = n - 2`,
  so the word is slot `level + 1 <= n - 1`. It is at least slot 1. For free, the level is 0, so
  slot 1.
- Slots `1..=level` hold its objects: `fill` writes slots `1..=level` and then records `level`;
  a magazine from a spare stack or the depot had its level recorded by whoever wrote its slots
  (`retire`, `serve_*`, `take_loaded`, `take_chain`, `launder`) after writing them. An empty
  magazine has none.

If the store happens, the slab now refers to the magazine and this thread no longer uses
`held` (the `Stored` arm never touches it again). If it does not, the thread still holds it.
