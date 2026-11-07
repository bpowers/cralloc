//! Whole routes from a free to the next allocation, on real threads. Every block carries a ledger
//! (`crate::stress::own`): a thread that allocates it must see all that the thread that freed it
//! wrote, however the block went between them. The ways through are the ones
//! `docs/research/memory-ordering.md` traces: the same CPU, a magazine through a depot stack, a
//! drain, and a launder and a fill.

use super::*;
use crate::stress::{FULL, LEDGER_BYTES, QUICK, Rounds, count, own, spread};
use core::sync::atomic::{AtomicPtr, AtomicU64, Ordering};
use std::collections::{BTreeSet, VecDeque};
use std::sync::Mutex;
use std::time::Duration;

/// A batch of blocks, as addresses a test can hand between threads.
type Batch = std::vec::Vec<usize>;

/// A pipeline: producers allocate blocks and take their ledgers, and consumers, on other CPUs,
/// take them again and free them, so that blocks go by magazines from the consumers to the
/// producers. Returns how many times blocks changed hands and the blocks that did, for the count
/// that catches a stale ledger.
fn pipeline<H: GlobalAlloc + Sync>(
    heap: &H,
    threads: usize,
    time: Duration,
    size: usize,
) -> (u64, BTreeSet<usize>) {
    assert!(size >= LEDGER_BYTES);
    let l = layout(size, 8);
    let batch = if cfg!(miri) { 8 } else { 64 };
    let queue = Mutex::new(VecDeque::<Batch>::new());
    let rounds = Rounds::of(time, 40);
    let done = AtomicU64::new(0);
    let results = spread(threads, |me| {
        let (mut owned, mut blocks) = (0u64, BTreeSet::new());
        if me.is_multiple_of(2) {
            let mut round = 0;
            while rounds.go_on(round) {
                round += 1;
                if queue.lock().unwrap().len() > 2 * threads {
                    std::thread::yield_now();
                    continue;
                }
                let made: Batch = (0..batch)
                    .map(|_| {
                        // SAFETY: a valid layout.
                        let p = unsafe { heap.alloc(l) };
                        assert!(!p.is_null());
                        // SAFETY: this thread's block, of at least the ledger's size.
                        unsafe { own(p) };
                        owned += 1;
                        p.expose_provenance()
                    })
                    .collect();
                queue.lock().unwrap().push_back(made);
            }
            done.fetch_add(1, Ordering::SeqCst);
        } else {
            let producers = threads.div_ceil(2) as u64;
            loop {
                let taken = queue.lock().unwrap().pop_front();
                let Some(taken) = taken else {
                    if done.load(Ordering::SeqCst) == producers {
                        break;
                    }
                    std::thread::yield_now();
                    continue;
                };
                for address in taken {
                    let p = ptr::with_exposed_provenance_mut::<u8>(address);
                    // SAFETY: the producer gave the block up, through the queue.
                    unsafe { own(p) };
                    owned += 1;
                    blocks.insert(address);
                    // SAFETY: allocated with this layout, freed once.
                    unsafe { heap.dealloc(p, l) };
                }
            }
        }
        (owned, blocks)
    });
    let owned = results.iter().map(|r| r.0).sum();
    let blocks = results.into_iter().flat_map(|r| r.1).collect();
    (owned, blocks)
}

/// Checks the count of every block's ledger against the number of takes.
fn counted_as_owned(owned: u64, blocks: &BTreeSet<usize>) {
    let counted: u64 = blocks
        .iter()
        // SAFETY: blocks of the heap, kept resident by the caller's faults, and nothing runs.
        .map(|&a| unsafe { count(ptr::with_exposed_provenance_mut(a)) })
        .sum();
    assert_eq!(counted, owned, "a new owner read a stale ledger");
}

fn blocks_keep_their_last_owners_writes(time: Duration, drains: bool) {
    let heap = SimHeap::<crate::os::sim::Standard>::new();
    // A purge reads as zero, and the count would not add up.
    heap.os().set_faults(Call::Purge, Faults::ALWAYS);
    let run = || pipeline(&heap, 4, time, 512);
    let (owned, blocks) = if drains {
        with_drains_running(&heap, run)
    } else {
        run()
    };
    counted_as_owned(owned, &blocks);
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

/// A program may pass a block to another thread with a relaxed store and have that thread free
/// it, if neither touches it after: C and Rust ask a free to be ordered only after the block's
/// last use, not after its allocation. The free then runs with no happens-before edge from
/// anything that made the block. On a fresh heap each round, so that the first block's free races
/// the heap's own start.
fn a_block_passed_by_a_relaxed_store_can_be_freed(time: Duration) {
    let rounds = Rounds::of(time, 3);
    let mut round = 0;
    while rounds.go_on(round) {
        round += 1;
        let heap = SimHeap::<crate::os::sim::Standard>::new();
        let passed = AtomicPtr::<u8>::new(ptr::null_mut());
        let l = layout(48, 8);
        spread(2, |me| {
            if me == 0 {
                // SAFETY: a valid layout.
                let p = unsafe { heap.alloc(l) };
                assert!(!p.is_null());
                passed.store(p, Ordering::Relaxed);
            } else {
                let p = loop {
                    let p = passed.load(Ordering::Relaxed);
                    if !p.is_null() {
                        break p;
                    }
                    core::hint::spin_loop();
                };
                // SAFETY: allocated with this layout, freed once.
                unsafe { heap.dealloc(p, l) };
            }
        });
    }
}

#[test]
fn a_block_passed_by_a_relaxed_store_can_be_freed_quickly() {
    a_block_passed_by_a_relaxed_store_can_be_freed(QUICK);
}

#[test]
#[ignore = "slow: run with scripts/gate --all"]
fn a_block_passed_by_a_relaxed_store_can_be_freed_fully() {
    a_block_passed_by_a_relaxed_store_can_be_freed(FULL);
}

/// The real routines on the real kernel: plain loads and stores on the fast paths, and
/// `membarrier` for the drains `collect` makes while the pipeline runs. A purge can zero a free
/// block, so the count at the end does not hold here; the ledgers' two words do.
#[cfg(cralloc_real)]
mod process {
    use super::*;
    use crate::Cralloc;
    use core::sync::atomic::AtomicBool;

    fn blocks_keep_their_last_owners_writes_on_the_real_routines(time: Duration) {
        let stop = AtomicBool::new(false);
        thread::scope(|scope| {
            scope.spawn(|| {
                while !stop.load(Ordering::SeqCst) {
                    Cralloc::collect();
                    thread::sleep(Duration::from_millis(1));
                }
            });
            let _ = pipeline(&Cralloc, 4, time, 512);
            stop.store(true, Ordering::SeqCst);
        });
    }

    #[test]
    fn blocks_keep_their_last_owners_writes_on_the_real_routines_quickly() {
        blocks_keep_their_last_owners_writes_on_the_real_routines(QUICK);
    }

    #[test]
    #[ignore = "slow: run with scripts/gate --all"]
    fn blocks_keep_their_last_owners_writes_on_the_real_routines_fully() {
        blocks_keep_their_last_owners_writes_on_the_real_routines(FULL);
    }
}
