# Safety ledger: `crates/cralloc/src/depot/generation.rs`

The process generation lives in the first word of the reservation's process page
(`Plan::process_page_offset`, `Space::process_page`), which `prepare` commits and marks to be
wiped on fork through `Space::commit` and `Space::wipe_on_fork`, both safe.

## GENERATION-01: `&*page.as_ptr().cast::<AtomicU64>()` in `word`

**Preconditions.** The address is aligned to 8, readable and writable for the lifetime of the
reference, and accessed only atomically while the reference is used.

**Proof.** `word` is called only from `current`, with the pointer `Area::generation_page`
returns, which is `Space::process_page()`. The process page is page-aligned
(`plans_keep_their_parts_apart` in `space::verify`), so aligned to 8. `current` calls it only
after reading the `READY` status bit with acquire ordering, and `prepare` sets `READY` (release,
in a `fetch_or`) only after `Space::commit` of that page succeeded. The `READY` bit and the page
belong to the same heap because of `Cache::new`'s contract: a depot is only ever used with the
one `Space` whose magazine area it carves, so the bit `current` reads was set by a `prepare`
that committed this space's process page, not another's. Committed memory of a live
reservation stays writable until unmapped, and the process page is never unmapped or purged, so
the reference may live as long as the reservation, which the caller's borrow of the `Space`
bounds. Nothing else lives on the page: magazines, metadata and objects are elsewhere in the
plan. The only accesses to the word are through this atomic view. A simulated fork zeroes the
page with plain writes (`SimOs::wipe_for_fork`), but only while every other simulated thread is
stopped; a real fork wipes the child's copy, which no thread of the parent can touch.

A fork child inherits `READY` and the mapping under it: the bit is set after the commit returned,
and the page stays writable in the child. It reads the word as zero and installs its own when it
first needs one, or, where the wipe failed, keeps its parent's and has drains off. Either is a word
of this page, so nothing here depends on when a generation is drawn.

`AtomicU64` here is `front::sync::AtomicU64`, which is `core`'s type, or in this crate's tests a
transparent wrapper of it, so the layout is that of a `u64` either way.

Reviewed: 5a8ade1