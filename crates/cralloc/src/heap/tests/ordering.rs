//! Whole routes from a free to the next allocation, on real threads. Every block carries a ledger
//! (`crate::stress::own`): a thread that allocates it must see all that the thread that freed it
//! wrote, however the block went between them. The ways through are the ones
//! `docs/research/memory-ordering.md` traces: the same CPU, a magazine through a depot stack, a
//! drain, and a launder and a fill.

use super::*;
use crate::stress::{FULL, LEDGER_BYTES, QUICK, Rounds, count, own, spread, together};
use core::sync::atomic::{AtomicBool, AtomicPtr, Ordering};
use std::collections::BTreeMap;
use std::time::Duration;

/// The way from a producer to its consumer: a ring of slots, null when empty. A slot's store is a
/// release and its load an acquire, so a consumer sees what the producer wrote. The other way a
/// producer reads a slot only to see that it is empty, with no order, so that nothing outside the
/// allocator carries a consumer's writes to a producer. A lock here would: its unlock after a
/// consumer's frees orders them before a producer's next lock.
struct Ring {
    slots: std::vec::Vec<AtomicPtr<u8>>,
    over: AtomicBool,
}

impl Ring {
    fn new() -> Ring {
        let slots = if cfg!(miri) { 16 } else { 1024 };
        Ring {
            slots: (0..slots)
                .map(|_| AtomicPtr::new(ptr::null_mut()))
                .collect(),
            over: AtomicBool::new(false),
        }
    }
}

/// A block seen by a consumer, kept for the count at the end.
struct Seen(*mut u8);
// SAFETY: an address and its provenance, read only when no thread runs.
unsafe impl Send for Seen {}

/// The blocks that changed hands, by address, and how many times they did.
type Owned = (u64, BTreeMap<usize, Seen>);

/// A pipeline: producers allocate blocks and take their ledgers, and consumers take them again and
/// free them, so that blocks go by magazines from the consumers back to the producers. `place` is
/// [`spread`] or [`together`]. Returns what the count that catches a stale ledger needs.
fn pipeline<H: GlobalAlloc + Sync>(
    heap: &H,
    place: fn(usize, &(dyn Fn(usize) -> Owned + Sync)) -> std::vec::Vec<Owned>,
    pairs: usize,
    time: Duration,
    size: usize,
) -> Owned {
    assert!(size >= LEDGER_BYTES);
    let l = layout(size, 8);
    let rings: std::vec::Vec<Ring> = (0..pairs).map(|_| Ring::new()).collect();
    let rounds = Rounds::of(time, 200);
    let body = |me: usize| {
        let ring = &rings[me / 2];
        let (mut owned, mut blocks) = (0u64, BTreeMap::new());
        let (mut at, mut round) = (0, 0);
        if me.is_multiple_of(2) {
            while rounds.go_on(round) {
                round += 1;
                let slot = &ring.slots[at];
                if !slot.load(Ordering::Relaxed).is_null() {
                    std::thread::yield_now();
                    continue;
                }
                // SAFETY: a valid layout.
                let p = unsafe { heap.alloc(l) };
                assert!(!p.is_null());
                // SAFETY: this thread's block, of at least the ledger's size. What the block's last
                // owner wrote has come only through the allocator.
                unsafe { own(p) };
                owned += 1;
                slot.store(p, Ordering::Release);
                at = (at + 1) % ring.slots.len();
            }
            ring.over.store(true, Ordering::Release);
        } else {
            loop {
                let over = ring.over.load(Ordering::Acquire);
                let slot = &ring.slots[at];
                let p = slot.load(Ordering::Acquire);
                if p.is_null() {
                    if over {
                        break;
                    }
                    // For the move to the next CPU, not to stop.
                    let _ = rounds.go_on(round);
                    round += 1;
                    std::thread::yield_now();
                    continue;
                }
                slot.store(ptr::null_mut(), Ordering::Relaxed);
                at = (at + 1) % ring.slots.len();
                // SAFETY: the producer gave the block up, through the ring.
                unsafe { own(p) };
                owned += 1;
                blocks.insert(p.addr(), Seen(p));
                // SAFETY: allocated with this layout, freed once.
                unsafe { heap.dealloc(p, l) };
            }
        }
        (owned, blocks)
    };
    let results = place(2 * pairs, &body);
    let owned = results.iter().map(|r| r.0).sum();
    let blocks = results.into_iter().flat_map(|r| r.1).collect();
    (owned, blocks)
}

/// Checks the count of every block's ledger against the number of takes.
fn counted_as_owned((owned, blocks): &Owned) {
    let counted: u64 = blocks
        .values()
        // SAFETY: blocks of the heap, kept resident by the caller's faults, and nothing runs.
        .map(|block| unsafe { count(block.0) })
        .sum();
    assert_eq!(counted, *owned, "a new owner read a stale ledger");
}

fn blocks_keep_their_last_owners_writes(time: Duration, drains: bool) {
    let heap = SimHeap::<crate::os::sim::Standard>::new();
    // A purge reads as zero, and the count would not add up.
    heap.os().set_faults(Call::Purge, Faults::ALWAYS);
    let run = || pipeline(&heap, |n, f| spread(n, f), 2, time, 512);
    let owned = if drains {
        with_drains_running(&heap, run)
    } else {
        run()
    };
    counted_as_owned(&owned);
}

#[test]
fn blocks_keep_their_last_owners_writes_through_magazines_quickly() {
    blocks_keep_their_last_owners_writes(QUICK, false);
}

#[test]
fn blocks_keep_their_last_owners_writes_through_drains_quickly() {
    blocks_keep_their_last_owners_writes(QUICK, true);
}

#[test]
#[ignore = "slow: run with scripts/gate --all"]
fn blocks_keep_their_last_owners_writes_through_magazines_fully() {
    blocks_keep_their_last_owners_writes(FULL, false);
}

#[test]
#[ignore = "slow: run with scripts/gate --all"]
fn blocks_keep_their_last_owners_writes_through_drains_fully() {
    blocks_keep_their_last_owners_writes(FULL, true);
}

/// The real routines on the real kernel: plain loads and stores on the fast paths, and
/// `membarrier` for the drains `collect` makes while the pipeline runs. A purge can zero a free
/// block, so the count at the end does not hold here; the ledgers' two words do.
#[cfg(cralloc_real)]
mod process {
    use super::*;
    use crate::Cralloc;

    fn on_the_real_routines(
        place: fn(usize, &(dyn Fn(usize) -> Owned + Sync)) -> std::vec::Vec<Owned>,
        time: Duration,
    ) {
        let stop = AtomicBool::new(false);
        thread::scope(|scope| {
            scope.spawn(|| {
                while !stop.load(Ordering::SeqCst) {
                    Cralloc::collect();
                    thread::sleep(Duration::from_millis(1));
                }
            });
            let _ = pipeline(&Cralloc, place, 2, time, 512);
            stop.store(true, Ordering::SeqCst);
        });
    }

    #[test]
    fn blocks_keep_their_last_owners_writes_on_the_real_routines_quickly() {
        on_the_real_routines(|n, f| spread(n, f), QUICK);
    }

    #[test]
    #[ignore = "slow: run with scripts/gate --all"]
    fn blocks_keep_their_last_owners_writes_on_the_real_routines_fully() {
        on_the_real_routines(|n, f| spread(n, f), FULL);
    }

    /// Threads taking turns on one CPU: a block's only way from a free to the next allocation is
    /// the routines' plain stores and loads, and the context switch between them.
    #[test]
    fn blocks_keep_their_last_owners_writes_on_one_cpu_quickly() {
        on_the_real_routines(|n, f| together(n, f), QUICK);
    }

    #[test]
    #[ignore = "slow: run with scripts/gate --all"]
    fn blocks_keep_their_last_owners_writes_on_one_cpu_fully() {
        on_the_real_routines(|n, f| together(n, f), FULL);
    }
}
