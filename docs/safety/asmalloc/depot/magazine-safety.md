# Safety ledger: `crates/asmalloc/src/depot/magazine.rs`

## MAGAZINE-01: `AtomicPtr::from_ptr` in `Magazine::slot`

**Preconditions of `AtomicPtr::from_ptr`.** The address is aligned to 8, valid for reads and
writes for the lifetime of the returned reference, and not accessed non-atomically while the
reference is used.

**Invariant of `Magazine`.** `base` is the magazine area's base pointer advanced by an offset
that `admits` accepted against the committed mark when the handle was made: at or past the
generation page, a multiple of the magazine's size, and with the whole magazine inside the first
`committed` bytes of the area. Only `Area::magazine_at` builds a `Magazine`, and it checks
exactly this. `Magazine` has no public constructor and its fields are private.

**The binding.** An `Area` pairs a `Space` with the `Depot` whose `carved` and `committed`
counters describe that space's magazine area. It is made only by `Cache::with_tuning`, whose
contract (CACHE-01 below) is that the depot is only ever used with that space. Without it, one
depot's committed mark could admit uncommitted memory of another space, or two depots could
carve the same magazines.

**Proof.**

- *In bounds.* `slot` masks the index with `slots - 1`, so it is below the slot count, and
  `depot::verify::an_admitted_magazine_lies_inside_the_committed_area` proves (for every size,
  offset, start, committed mark and slot) that an admitted magazine's slots lie between the start
  and the committed mark. The committed mark only grows (`fetch_max`), and only after the memory
  under it was committed successfully (`Area::commit_to`, `Area::generation_page`), so it was
  committed when the handle was made and still is.
- *Valid for the lifetime.* Committed memory of a live reservation stays readable and writable
  until it is unmapped (the `Os` contract). A `Magazine<'a>` is made only by an `Area<'a, O>`,
  which holds a `Space<'a, O>`, so the borrow checker keeps the handle from outliving the
  `Space`, and so the OS value that owns the reservation (a `SpaceCell` unmaps only when dropped,
  which needs `&mut` and so no live `Space`). Nothing in the crate purges or unmaps the magazine
  area while a `Space` exists. The returned reference borrows the handle, so it lives no longer.
- *Provenance.* `base` comes from `Space::magazines()` through `map_addr`, so it carries the
  reservation's provenance.
- *Aligned.* The area is aligned to `Geometry::magazine_area_align`, at least the largest
  magazine, and the offset is a multiple of the magazine's size, so the base is aligned to the
  magazine's size (at least 32) and every slot to 8.
- *No non-atomic access.* Every access to magazine memory in the crate is atomic: this
  function's views, the twins' (`front/twin.rs`), and the routines' single `mov`s, which are
  atomic for aligned words on x86-64.

What a slot holds is not a memory-safety matter here: the middle tier never dereferences an
object pointer, and a wrong link or level is caught by `admits` the next time it is used as an
address, which makes the code abort rather than reach outside the area.

## CACHE-01: `Cache::with_tuning` in `Cache::new` (`crates/asmalloc/src/depot/mod.rs`)

**Proof.** `Cache::new` has the same contract as `Cache::with_tuning` and passes its caller's
promise on unchanged. The contract itself is what MAGAZINE-01 (the committed mark describes this
space) and TWIN-01 to TWIN-03 (slab words and spare links name this area's magazines) rely on.
