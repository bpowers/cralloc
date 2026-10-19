//! EXPERIMENT (domain-affine back end): counters of where fills and depot pops get their
//! objects, by the cache domain of the thread. Only with the `x-count` feature; every call is a
//! relaxed add on a line of the domain's own, so a counting build is for counts, not times.

use core::sync::atomic::AtomicU64;
use core::sync::atomic::AtomicU8;
use core::sync::atomic::Ordering::Relaxed;

/// Domains counted apart; others fold in.
pub const DOMAINS: usize = 16;

/// What is counted, per domain.
#[derive(Clone, Copy)]
#[repr(usize)]
pub enum Event {
    /// Calls of the back end's fill from a magazine slow path.
    FillCalls,
    /// Objects a fill claimed from free slots in a region of the filler's domain.
    FillOwn,
    /// Objects a fill claimed from free slots in a region of another domain.
    FillOther,
    /// Objects a fill carved.
    FillCarved,
    /// Full magazines popped from the domain's stack.
    PopDomain,
    /// Full magazines popped from the shared stack.
    PopShared,
    /// ... of which another domain pushed.
    PopSharedOther,
    /// Objects in full magazines popped from the shared stack that another domain pushed.
    PopSharedOtherObjects,
    /// Empty magazines popped from the shared empty stack.
    PopEmptyShared,
    /// ... of which another domain pushed.
    PopEmptySharedOther,
    /// Full magazines pushed onto the shared stack.
    PushShared,
    /// Objects laundered into the bitmaps.
    Laundered,
    /// Fills that took own-domain slots before a shared pop (`x-own-fill-first`).
    OwnFirst,
    /// Objects those fills took.
    OwnFirstObjects,
}

/// Number of events.
pub const EVENTS: usize = 14;

#[repr(C, align(128))]
struct Line([AtomicU64; 16]);

static COUNTS: [Line; DOMAINS] = [const { Line([const { AtomicU64::new(0) }; 16]) }; DOMAINS];

/// The domain plus one that last pushed each magazine to a shared stack, by magazine number.
static PUSHER: [AtomicU8; 1 << 20] = [const { AtomicU8::new(0) }; 1 << 20];

/// Adds `n` to `event` of `domain`.
#[inline]
pub fn add(domain: usize, event: Event, n: u64) {
    COUNTS[domain % DOMAINS].0[event as usize].fetch_add(n, Relaxed);
}

/// Records that `domain` pushed magazine `id` to a shared stack.
#[inline]
pub fn pushed(id: u32, domain: usize) {
    PUSHER[id as usize % PUSHER.len()].store((domain % DOMAINS) as u8 + 1, Relaxed);
}

/// Whether magazine `id`, popped from a shared stack by `domain`, was pushed by another.
#[inline]
pub fn from_other(id: u32, domain: usize) -> bool {
    let p = PUSHER[id as usize % PUSHER.len()].load(Relaxed);
    p != 0 && p as usize - 1 != domain % DOMAINS
}

/// Every count, `[domain][event]`.
pub fn snapshot() -> [[u64; EVENTS]; DOMAINS] {
    let mut out = [[0; EVENTS]; DOMAINS];
    for (d, line) in COUNTS.iter().enumerate() {
        for (e, v) in out[d].iter_mut().enumerate() {
            *v = line.0[e].load(Relaxed);
        }
    }
    out
}

/// The events' names, in order.
pub const NAMES: [&str; EVENTS] = [
    "fill_calls",
    "fill_own",
    "fill_other",
    "fill_carved",
    "pop_domain",
    "pop_shared",
    "pop_shared_other",
    "pop_shared_other_objects",
    "pop_empty_shared",
    "pop_empty_shared_other",
    "push_shared",
    "laundered",
    "own_first",
    "own_first_objects",
];
