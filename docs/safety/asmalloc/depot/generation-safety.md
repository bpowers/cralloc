# Safety ledger: `crates/asmalloc/src/depot/generation.rs`

The process generation lives in the first word of the magazine area's first page, which is
marked to be wiped on fork. Until the foundation gives the plan a page of its own, the page is
reached through the magazine area.

## GENERATION-01: `&*page.as_ptr().cast::<AtomicU64>()` in `word`

**Preconditions.** The address is aligned to 8, readable and writable for the lifetime of the
reference, and accessed only atomically while the reference is used.

**Proof.** `word` is called only with the pointer `Area::generation_page` returns, which is the
start of the magazine area, `Space::magazines()`. That is aligned to
`Geometry::magazine_area_align` (at least the page size), so to 8. `generation_page` returns it
only after the page is known to be committed: either it committed the page itself and the call
succeeded, or the committed mark is not zero, and the mark is raised only after the memory under
it has been committed, starting from offset 0. Committed memory of a live reservation stays
writable until unmapped, and the magazine area is never unmapped or purged, so the reference may
live as long as the reservation, which the caller's borrow of the `Space` bounds. Magazines start
after the page (`Area::start`), so no magazine slot overlaps the word. The only accesses to the
word are through this atomic view. A simulated fork zeroes the page with plain writes
(`SimOs::wipe_for_fork`), but only while every other simulated thread is stopped; a real fork
wipes the child's copy, which no thread of the parent can touch.

`AtomicU64` here is `front::sync::AtomicU64`, which is `core`'s type, or in this crate's tests a
transparent wrapper of it, so the layout is that of a `u64` either way.

## GENERATION-02: `space.os().wipe_on_fork(page, len)` in `prepare`

**Preconditions of `Os::wipe_on_fork`.** The range is page-aligned, a whole number of pages, not
empty, and inside one live reservation made by the same `Os` value.

**Proof.** `page` is the start of the magazine area, which is page-aligned
(`plans_keep_objects_metadata_and_magazines_apart` in `space::verify`), and `len` is one page.
The magazine area is at least one largest magazine and at least one page
(`Geometry::try_new` requires `page_shift <= magazine_shift`), so the page lies inside the
reservation of the `Space` the area was made from, which is live while the `Space` is borrowed,
and the call goes to that `Space`'s own OS. This block goes away when `Space` gains a checked
wrapper for the call.
