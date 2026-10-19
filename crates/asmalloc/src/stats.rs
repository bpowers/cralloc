//! Read-only introspection: where the heap's memory is.
//!
//! [`Asmalloc::inspect`](crate::Asmalloc::inspect) walks the back end's regions and bitmaps, the
//! per-CPU slabs and the depot, and tells a [`Visitor`] what it finds: every region with how
//! far it is carved and committed, every run of free slots in the bitmaps, and every object
//! parked in a magazine with where the magazine is. The allocator knows what memory is for; only
//! the kernel knows what is resident, so a visitor that wants resident bytes asks the kernel
//! about the address ranges it is told (`mincore`, `/proc/self/pagemap`) and intersects.
//!
//! The walk writes no shared state, takes nothing, never allocates and never waits. It runs
//! beside threads that are allocating, so what it reports is a racy snapshot: an object may be
//! seen twice or not at all as it moves, and a magazine chain may be cut short. Every address it
//! follows is checked before use (the same checks the slow paths make), so a stale link costs
//! accuracy, never safety. It contains no `unsafe`.
//!
//! The walks themselves live with the structures they read: `back/inspect.rs` and
//! `depot/inspect.rs`.

pub use crate::classes::{Class, MagazineSize, SmallClass};

/// Where a magazine is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Place {
    /// Loaded in this CPU's slab: what the fast paths pop from and push to.
    Loaded(u32),
    /// On one of this CPU's spare stacks.
    Spare(u32),
    /// On the depot's stack for its class.
    Depot,
}

/// One region assigned to a class.
#[derive(Clone, Copy, Debug)]
pub struct Region {
    /// The class whose slots it holds.
    pub class: Class,
    /// The region number.
    pub index: usize,
    /// Its first byte.
    pub base: *const u8,
    /// Its size in bytes: the heap's region size.
    pub len: usize,
    /// Where the watermark is, in bytes from the base: the slots below it have been handed out
    /// at least once. A small class's slots start a few pages past the base (its origin), and
    /// nothing below the first slot is ever written or made writable.
    pub carved: usize,
    /// Where the writable part ends, in bytes from the base. It starts at the class's first
    /// slot.
    pub committed: usize,
    /// The byte offset at which the huge-page part begins, if the region has one.
    pub huge_from: Option<usize>,
    /// The region's metadata block.
    pub metadata: *const u8,
    /// Bytes of the metadata block the class's layout uses.
    pub metadata_len: usize,
}

/// What a walk reports. Every method has an empty default, so a visitor implements what it
/// wants. Calls come in this order: `retained`; then for each region `region` followed by its
/// `free` runs, or `run`; then `magazine_area` and `slabs`; then, for each magazine with
/// objects, `magazine` followed by `cached` for each object in it; then `empties`.
pub trait Visitor {
    /// The back end's count of free bytes in the bitmaps that it believes are resident: what
    /// the retained cap limits.
    fn retained(&mut self, bytes: usize) {
        let _ = bytes;
    }

    /// A region assigned to a class.
    fn region(&mut self, region: &Region) {
        let _ = region;
    }

    /// A run of consecutive free slots in the bitmap of the region last reported: `len` bytes
    /// at `start`. A slot being purged or claimed at this moment is not reported.
    fn free(&mut self, class: Class, start: *const u8, len: usize) {
        let _ = (class, start, len);
    }

    /// An object bigger than a region: `len` bytes of whole regions at `base`, of which the
    /// program asked for `size`.
    fn run(&mut self, base: *const u8, len: usize, size: usize) {
        let _ = (base, len, size);
    }

    /// The magazine area: `carved` bytes from `base` hold magazines, `committed` are writable.
    fn magazine_area(&mut self, base: *const u8, carved: usize, committed: usize) {
        let _ = (base, carved, committed);
    }

    /// The slab array: `count` slabs of `bytes` each from `base` belong to CPU numbers a slow
    /// path has run on.
    fn slabs(&mut self, base: *const u8, bytes: usize, count: usize) {
        let _ = (base, bytes, count);
    }

    /// A magazine at `place` holding `objects` objects of `class` (possibly none).
    fn magazine(&mut self, place: Place, class: SmallClass, objects: usize) {
        let _ = (place, class, objects);
    }

    /// One object in the magazine last reported.
    fn cached(&mut self, place: Place, class: SmallClass, object: *const u8) {
        let _ = (place, class, object);
    }

    /// The depot's stack of empty magazines of `size` holds `count`.
    fn empties(&mut self, size: MagazineSize, count: usize) {
        let _ = (size, count);
    }
}
