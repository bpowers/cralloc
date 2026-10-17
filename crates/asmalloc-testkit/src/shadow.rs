//! The address index of live blocks, for the overlap check.
//!
//! Everything else the model knows about a block travels with the block (see
//! [`crate::model::Live`]); only the question "does this new block overlap any live block,
//! whoever holds it?" needs shared state. One lock around one map would make every operation
//! of every thread wait on every other, which hides exactly the races a multithreaded test is
//! for. So the index is sharded by address: memory is cut into 64 KiB granules, each granule
//! hashes to one of 64 shards, and a block is recorded in the shard of every granule it
//! touches. Two blocks that overlap share a granule, so both are recorded in that granule's
//! shard and the second is caught there. Blocks in different places take different locks.
//!
//! Each shard holds disjoint intervals keyed by start address, so the only one that can reach
//! into `[start, end)` is the last one starting below `end`: one ordered lookup per shard.
//!
//! The model claims a block after the allocator returns it and releases it before handing it
//! back, so the index never holds a block the allocator considers free. The shard locks still
//! order things: two threads that touch the same shard get a happens-before edge between the
//! allocator calls around those touches, and each lock is a fence. Sharding makes that rare,
//! not absent, so a race checker (Miri, ThreadSanitizer) can miss an allocator race on memory
//! two threads used in turn. An index built on relaxed atomic read-modify-writes would conflict
//! without ordering; it is not built yet.

use std::collections::BTreeMap;
use std::sync::{Mutex, MutexGuard, PoisonError};

use crate::op::BlockId;

const SHARDS: usize = 64;
const GRANULE_SHIFT: u32 = 16;
/// Beyond this many granules a block is recorded in every shard without hashing each one.
const MAX_HASHED_GRANULES: usize = 4096;

/// A live interval in the index.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Interval {
    /// First byte.
    pub start: usize,
    /// One past the last byte.
    pub end: usize,
    /// The block.
    pub owner: BlockId,
}

type Shard = BTreeMap<usize, (usize, BlockId)>;

/// The sharded index of live blocks.
#[derive(Debug)]
pub struct Shadow {
    shards: Box<[Mutex<Shard>]>,
}

impl Default for Shadow {
    fn default() -> Self {
        Shadow::new()
    }
}

fn shard_of(granule: usize) -> usize {
    ((granule as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) >> (64 - SHARDS.trailing_zeros()))
        as usize
}

/// The shards `[start, end)` is recorded in, as a bit set.
fn shards_of(start: usize, end: usize) -> u64 {
    debug_assert!(start < end);
    let first = start >> GRANULE_SHIFT;
    let last = (end - 1) >> GRANULE_SHIFT;
    if last - first >= MAX_HASHED_GRANULES {
        return u64::MAX;
    }
    (first..=last).fold(0, |set, g| set | 1 << shard_of(g))
}

fn shard_indices(set: u64) -> impl Iterator<Item = usize> {
    (0..SHARDS).filter(move |i| set & (1 << i) != 0)
}

impl Shadow {
    /// An empty index.
    pub fn new() -> Self {
        Shadow {
            shards: (0..SHARDS).map(|_| Mutex::new(Shard::new())).collect(),
        }
    }

    fn lock(&self, i: usize) -> MutexGuard<'_, Shard> {
        // A panic while holding a shard leaves it consistent: every update is one map call.
        self.shards[i]
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Record `[start, end)` for `owner`, or report a live interval it overlaps (in which case
    /// nothing is recorded). `start < end`.
    pub fn claim(&self, start: usize, end: usize, owner: BlockId) -> Result<(), Interval> {
        let set = shards_of(start, end);
        let mut done = 0u64;
        for i in shard_indices(set) {
            let mut shard = self.lock(i);
            if let Some((&s, &(e, other))) = shard.range(..end).next_back()
                && e > start
            {
                drop(shard);
                self.remove_from(done, start, owner);
                return Err(Interval {
                    start: s,
                    end: e,
                    owner: other,
                });
            }
            shard.insert(start, (end, owner));
            done |= 1 << i;
        }
        Ok(())
    }

    /// Forget `[start, end)`, which `owner` claimed.
    pub fn release(&self, start: usize, end: usize, owner: BlockId) {
        self.remove_from(shards_of(start, end), start, owner);
    }

    fn remove_from(&self, set: u64, start: usize, owner: BlockId) {
        for i in shard_indices(set) {
            let removed = self.lock(i).remove(&start);
            assert!(
                matches!(removed, Some((_, o)) if o == owner),
                "model bug: block {owner} at {start:#x} was not claimed in shard {i}"
            );
        }
    }

    /// True when no interval is recorded.
    pub fn is_empty(&self) -> bool {
        (0..SHARDS).all(|i| self.lock(i).is_empty())
    }

    /// Every recorded interval, once each, in address order.
    pub fn intervals(&self) -> Vec<Interval> {
        let mut all: BTreeMap<usize, Interval> = BTreeMap::new();
        for i in 0..SHARDS {
            for (&start, &(end, owner)) in self.lock(i).iter() {
                all.insert(start, Interval { start, end, owner });
            }
        }
        all.into_values().collect()
    }

    /// A copy made in a child process after `fork`, keeping the intervals whose owner `keep`
    /// accepts. Threads that held a shard's lock at the moment of the fork do not exist in the
    /// child, so a locked shard is skipped rather than waited for. The copy can therefore miss
    /// intervals, which weakens the child's overlap check but never makes it report a false
    /// overlap: every interval was claimed after its block was allocated.
    pub fn copy_after_fork(&self, keep: impl Fn(BlockId) -> bool) -> Shadow {
        let copy = Shadow::new();
        for (i, shard) in self.shards.iter().enumerate() {
            let guard = match shard.try_lock() {
                Ok(g) => g,
                Err(std::sync::TryLockError::Poisoned(p)) => p.into_inner(),
                Err(std::sync::TryLockError::WouldBlock) => continue,
            };
            let mut dst = copy.lock(i);
            for (&start, &(end, owner)) in guard.iter() {
                if keep(owner) {
                    dst.insert(start, (end, owner));
                }
            }
        }
        copy
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const G: usize = 1 << GRANULE_SHIFT;

    #[test]
    fn disjoint_blocks_coexist_and_overlaps_are_caught_from_either_side() {
        let s = Shadow::new();
        s.claim(0x1000, 0x1010, BlockId(1)).unwrap();
        s.claim(0x1010, 0x1020, BlockId(2)).unwrap();
        s.claim(0x0ff0, 0x1000, BlockId(3)).unwrap();
        let hit = |a, b| s.claim(a, b, BlockId(9)).unwrap_err().owner;
        assert_eq!(hit(0x100f, 0x1011), BlockId(2));
        assert_eq!(hit(0x0fff, 0x1001), BlockId(1));
        assert_eq!(hit(0x0000, 0x2000), BlockId(2));
        assert_eq!(hit(0x1008, 0x1009), BlockId(1));
        assert_eq!(s.intervals().len(), 3, "failed claims record nothing");
        s.release(0x1010, 0x1020, BlockId(2));
        s.claim(0x1010, 0x1018, BlockId(4)).unwrap();
    }

    #[test]
    fn blocks_spanning_many_granules_are_found_from_any_of_them() {
        let s = Shadow::new();
        let (start, end) = (3 * G + 5, 40 * G + 7);
        s.claim(start, end, BlockId(1)).unwrap();
        for probe in [start, 10 * G, 17 * G + 3, end - 1] {
            assert_eq!(
                s.claim(probe, probe + 1, BlockId(2)).unwrap_err().owner,
                BlockId(1)
            );
        }
        s.claim(end, end + 1, BlockId(3)).unwrap();
        s.claim(start - 1, start, BlockId(4)).unwrap();
        let huge = (MAX_HASHED_GRANULES + 10) * G;
        s.claim(1 << 40, (1 << 40) + huge, BlockId(5)).unwrap();
        assert!(
            s.claim((1 << 40) + huge / 2, (1 << 40) + huge / 2 + 8, BlockId(6))
                .is_err()
        );
        for (a, b, id) in [(start, end, 1), (end, end + 1, 3), (start - 1, start, 4)] {
            s.release(a, b, BlockId(id));
        }
        s.release(1 << 40, (1 << 40) + huge, BlockId(5));
        assert!(s.is_empty());
    }

    #[test]
    fn concurrent_claims_of_one_range_admit_exactly_one() {
        for round in 0..20 {
            let s = Shadow::new();
            let base = round * 8 * G + 100;
            let winners: usize = std::thread::scope(|scope| {
                let handles: Vec<_> = (0..4)
                    .map(|t| {
                        let s = &s;
                        scope.spawn(move || {
                            usize::from(s.claim(base + t, base + 3 * G, BlockId(t as u64)).is_ok())
                        })
                    })
                    .collect();
                handles.into_iter().map(|h| h.join().unwrap()).sum()
            });
            assert_eq!(winners, 1);
        }
    }

    #[test]
    fn a_copy_keeps_only_the_chosen_owners() {
        let s = Shadow::new();
        s.claim(0x1000, 0x2000, BlockId(1)).unwrap();
        s.claim(0x5000, 0x6000, BlockId(2)).unwrap();
        let c = s.copy_after_fork(|o| o != BlockId(2));
        assert_eq!(
            c.intervals(),
            vec![Interval {
                start: 0x1000,
                end: 0x2000,
                owner: BlockId(1)
            }]
        );
    }
}
