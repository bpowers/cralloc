# Safety ledger: `crates/asmalloc/src/depot/magazine.rs`

## MAGAZINE-01: `AtomicPtr::from_ptr` in `Magazine::slot`

**Preconditions of `AtomicPtr::from_ptr`.** The address is aligned to 8, valid for reads and
writes for the lifetime of the returned reference, and not accessed non-atomically while the
reference is used.

**Invariant of `Magazine`.** `base` is the magazine area's base pointer advanced by an offset
that `admits` accepted against the committed mark when the handle was made: at or past the
first unit of the area, a multiple of the magazine's size, and with the whole magazine inside
the first `committed` bytes of the area. Only `Area::magazine_at` builds a `Magazine`, and it checks
exactly this. `Magazine` has no public constructor and its fields are private.

**The binding.** An `Area` pairs a `Space` with the `Depot` whose `carved` and `committed`
counters describe that space's magazine area. It is made only by `Cache::with_tuning`, whose
contract (CACHE-01, `depot/mod-safety.md`) is that the depot is only ever used with that space. Without it, one
depot's committed mark could admit uncommitted memory of another space, or two depots could
carve the same magazines.

**Proof.**

- *In bounds.* `slot` masks the index with `slots - 1`, so it is below the slot count, and
  `depot::verify::an_admitted_magazine_lies_inside_the_committed_area` proves (for every size,
  offset, start, committed mark and slot) that an admitted magazine's slots lie between the start
  and the committed mark. The committed mark only grows (`fetch_max`), and only after the memory
  under it was committed successfully (`Area::commit_to`), so it was committed when the handle
  was made and still is.
- *Valid for the lifetime.* Committed memory of a live reservation stays readable and writable
  until it is unmapped (the `Os` contract). A `Magazine<'a>` is made only by an `Area<'a, O>`,
  which holds a `Space<'a, O>`, so the borrow checker keeps the handle from outliving the
  `Space`, and so the OS value that owns the reservation (a `SpaceCell` unmaps only when dropped,
  which needs `&mut` and so no live `Space`). Nothing in the crate purges or unmaps the magazine
  area while a `Space` exists. The returned reference borrows the handle, so it lives no longer.
- *Provenance.* `base` comes from `Space::magazines()` through `map_addr`, so it carries the
  reservation's provenance.
- *Aligned.* The area is aligned to the region size (`Plan::magazines_offset`), which
  `Geometry::try_new` requires to be at least the largest magazine at every region size the
  geometry allows (`space::verify`, `plans_keep_their_parts_apart`), and the offset is a multiple
  of the magazine's size, so the base is aligned to the magazine's size (at least 32) and every
  slot to 8.
- *No non-atomic access.* Every access to magazine memory in the crate is atomic: this
  function's views, `Magazine::objects`' slice (MAGAZINE-02), through which the back end's
  fill and launder store and load with relaxed atomics, the twins' (`front/twin.rs`), and the
  routines' single `mov`s, which are atomic for aligned words on x86-64.

## MAGAZINE-02: `slice::from_raw_parts` in `Magazine::objects`

**Preconditions of `slice::from_raw_parts`.** The pointer is non-null and aligned for
`AtomicPtr<u8>`; the `capacity` elements from it lie in one allocation, are valid for reads
for the lifetime of the slice, and are initialized values of the type; nothing mutates them
through a non-atomic access while the slice is used; the total size is at most `isize::MAX`.

**Proof.** The elements are slots 1 to `capacity` of the magazine, that is slots 1 to
`slots - 2`, each of which `Magazine::slot` would also reach. So everything MAGAZINE-01 says of
a slot holds of each element: inside committed magazine memory of a live reservation (one
allocation: the reservation), aligned to 8, valid for the lifetime of the handle's borrow,
with the reservation's provenance. The slice borrows `self`, so it lives no longer than the
handle. `AtomicPtr<u8>` has the size and alignment of `*mut u8` and every bit pattern of a
word is a valid one (memory reads as zero until written, then holds what an atomic store
wrote), so the elements are initialized. Interior mutability through the slice is atomic, so
it does not conflict with the other atomic views of the same words (MAGAZINE-01's "No
non-atomic access" covers them all). The size is at most the largest magazine, a few KiB.
`front::sync::AtomicPtr` is a transparent wrapper of the same `core` type, so the views agree
in layout under the tests' checker too.

For both entries, what a slot holds is not a memory-safety matter: the middle tier never dereferences an
object pointer, and a wrong link or level is caught by `admits` the next time it is used as an
address, which makes the code abort rather than reach outside the area.

Reviewed: c658347
