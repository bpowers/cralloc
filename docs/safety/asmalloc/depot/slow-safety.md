# Safety ledger: `crates/asmalloc/src/depot/slow.rs`

## DEPOT-SLOW-01: `store_if_equal` in `exchange_for_alloc` and `exchange_for_free`, and the install of `centre_for_alloc` and `centre_for_free`

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

The installs of `centre_for_alloc` and `centre_for_free` pass `held.word(k)` for a `held` this
call holds, with `k` the level `Magazine::move_top_into` just recorded on it, after writing its
slots: for an allocation, `held` keeps slots `1..=level - half` of its own objects; for a free,
`held` was empty and receives slots `1..=half` from `old`, which this thread also holds (see
DEPOT-SLOW-02). Both levels are at most the capacity, so the word is a slot from 1 to `n - 1`.
Their expected word is zero, which is what DEPOT-SLOW-02 stored.

## DEPOT-SLOW-02: `store_if_equal` of zero in `centre_for_alloc` and `centre_for_free`

**Precondition.** `new` keeps the slab invariant: zero is a slab word with no magazine, which the
invariant allows and every routine reads as one.

**What the store gives the thread.** It commits only if the current CPU's word is still `word`,
which `classify` read and found to name slot `s` of the magazine `old`. By the slab invariant
that word was the only reference to `old`, so after the commit nothing refers to it and it is
this thread's, at the level the word gave it (as the age limit's take argues, AGE-01, including
for a thread that moved between the read and the routine). The thread then holds `held` and
`old`, neither loaded nor on a stack, and copies between them with `move_top_into` before either
is installed or retired. Between this store and the install the slab word is zero: a routine on
the CPU misses and its slow path may install its own magazine, or a drain may put a marker
there; the install then finds a word that is not zero and stores nothing, and the thread retires
or serves from the two magazines it still holds, as a failed install does.

Reviewed: f41dbf9
