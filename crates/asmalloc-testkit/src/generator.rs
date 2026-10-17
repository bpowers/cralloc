//! Turning entropy into operations.
//!
//! A [`Generator`] draws one operation at a time in the shape of a [`Profile`], keeping its own
//! record of which blocks are live so it can pick victims and stay inside the live-byte budget.
//! It is the same code whether the entropy is a seeded PRNG or a fuzzer's bytes, which is what
//! makes the fuzzer's input an operation stream. The model reports back what it observed
//! ([`Generator::observe`]): a zero-size allocation that returned nothing, a hand-off that did
//! not fit in the receiver's mailbox, blocks received from other workers.

use std::collections::HashMap;

use crate::entropy::Entropy;
use crate::model::Effect;
use crate::op::{BlockId, Op};
use crate::profile::{Growth, Order, Profile, Sizes};
use crate::subject::ZeroSize;

const KIB: usize = 1024;
const MIB: usize = 1024 * KIB;

/// Blocks of up to this size are "small" in [`Sizes::Bands`]; the bands match talc's and
/// wasmalloc's random-actions workloads.
const SMALL_MAX: usize = 10_000;

/// Where a worker's hand-offs go, and how often it sends and receives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Exchange {
    /// Number of workers in the run.
    pub workers: u32,
    /// This worker's index.
    pub me: u32,
    /// Weight of sending a block.
    pub send: u32,
    /// Weight of receiving blocks.
    pub recv: u32,
    /// Where sends go.
    pub target: Target,
    /// Percent of the profile's allocation weight this worker keeps (consumers allocate less).
    pub alloc_percent: u32,
    /// Percent of the profile's free weight this worker keeps (producers free less).
    pub free_percent: u32,
}

/// Where a worker sends blocks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    /// Always the same worker.
    Partner(u32),
    /// A random other worker.
    AnyOther,
    /// The worker's own queue, from which others steal.
    Own,
}

#[derive(Clone, Copy, Debug)]
struct Entry {
    id: BlockId,
    size: usize,
    align: usize,
}

/// Live blocks in allocation order, with constant-time lookup and random removal.
#[derive(Debug, Default)]
struct Mirror {
    order: Vec<Entry>,
    pos: HashMap<BlockId, usize>,
}

impl Mirror {
    fn len(&self) -> usize {
        self.order.len()
    }

    fn is_empty(&self) -> bool {
        self.order.is_empty()
    }

    fn push(&mut self, e: Entry) {
        self.pos.insert(e.id, self.order.len());
        self.order.push(e);
    }

    /// Remove the entry at `i`. Keeping order costs a shift, so only queue-ordered frees ask.
    fn remove_at(&mut self, i: usize, keep_order: bool) -> Entry {
        let e = if keep_order {
            let e = self.order.remove(i);
            for (j, moved) in self.order[i..].iter().enumerate() {
                self.pos.insert(moved.id, i + j);
            }
            e
        } else {
            let e = self.order.swap_remove(i);
            if let Some(moved) = self.order.get(i) {
                self.pos.insert(moved.id, i);
            }
            e
        };
        self.pos.remove(&e.id);
        e
    }

    fn remove_id(&mut self, id: BlockId) -> Option<Entry> {
        let i = *self.pos.get(&id)?;
        Some(self.remove_at(i, true))
    }
}

const ALLOC: usize = 0;
const ZEROED: usize = 1;
const FREE: usize = 2;
const RESIZE: usize = 3;
const TOUCH: usize = 4;
const MIGRATE: usize = 5;
const SEND: usize = 6;
const RECV: usize = 7;

/// Draws operations in the shape of a profile.
#[derive(Debug)]
pub struct Generator {
    profile: Profile,
    zero: ZeroSize,
    exchange: Option<Exchange>,
    budget: usize,
    next_id: u64,
    live: Mirror,
    live_bytes: usize,
    ops: usize,
    sweeping: bool,
    batch_left: usize,
    free_left: usize,
    last_sent: Option<Entry>,
}

impl Generator {
    /// A generator whose blocks are numbered from `first_id`, keeping at most `budget` bytes
    /// live, for a subject that allows the zero sizes in `zero`.
    pub fn new(
        profile: Profile,
        zero: ZeroSize,
        first_id: u64,
        budget: usize,
        exchange: Option<Exchange>,
    ) -> Self {
        profile.validate();
        if let Some(x) = exchange {
            assert!(x.me < x.workers, "worker {} of {}", x.me, x.workers);
            assert!(
                x.target != Target::AnyOther || x.workers > 1,
                "sending to another worker needs two"
            );
        }
        Generator {
            profile,
            zero,
            exchange,
            budget: budget.max(1),
            next_id: first_id,
            live: Mirror::default(),
            live_bytes: 0,
            ops: 0,
            sweeping: false,
            batch_left: 0,
            free_left: 0,
            last_sent: None,
        }
    }

    /// Blocks the generator believes are live.
    pub fn live_blocks(&self) -> usize {
        self.live.len()
    }

    /// The next operation, or `None` once the entropy runs dry.
    pub fn next_op<E: Entropy>(&mut self, e: &mut E) -> Option<Op> {
        self.last_sent = None;
        self.ops += 1;
        let p = self.profile;
        if p.sweep_every > 0 && self.ops.is_multiple_of(p.sweep_every) {
            self.sweeping = true;
        }
        if self.sweeping {
            if !self.live.is_empty() {
                return self.free_op(e);
            }
            self.sweeping = false;
        }
        if p.batch > 0 {
            if self.free_left > 0 && !self.live.is_empty() {
                self.free_left -= 1;
                return self.free_op(e);
            }
            self.free_left = 0;
            if self.batch_left == 0 {
                self.batch_left = e.range(1, p.batch)?;
            }
        }
        match e.weighted(&self.weights())? {
            ALLOC => self.alloc_op(e, false),
            ZEROED => self.alloc_op(e, true),
            FREE => self.free_op(e),
            RESIZE => self.resize_op(e),
            TOUCH => self.touch_op(e),
            MIGRATE => Some(Op::Migrate),
            SEND => self.send_op(e),
            RECV => self.recv_op(e),
            _ => unreachable!("eight weights"),
        }
    }

    /// Update the generator's record with what the model observed.
    pub fn observe(&mut self, effect: &Effect) {
        match effect {
            Effect::Gone(id) => {
                if let Some(e) = self.live.remove_id(*id) {
                    self.live_bytes -= e.size;
                }
            }
            Effect::Kept => {
                if let Some(e) = self.last_sent.take() {
                    self.live_bytes += e.size;
                    self.live.push(e);
                }
            }
            Effect::Adopted(blocks) => {
                for b in blocks {
                    self.live_bytes += b.size;
                    self.live.push(Entry {
                        id: b.id,
                        size: b.size,
                        align: b.align,
                    });
                }
            }
            Effect::None | Effect::Migrate => {}
        }
        self.last_sent = None;
    }

    fn weights(&self) -> [u32; 8] {
        let w = self.profile.weights;
        let (mut alloc, mut zeroed, mut free) = (w.alloc, w.zeroed, w.free);
        let (mut send, mut recv) = (0, 0);
        if let Some(x) = self.exchange {
            alloc = alloc * x.alloc_percent / 100;
            zeroed = zeroed * x.alloc_percent / 100;
            free = free * x.free_percent / 100;
            (send, recv) = (x.send, x.recv);
        }
        let mut k = [
            alloc, zeroed, free, w.resize, w.touch, w.migrate, send, recv,
        ];
        if self.profile.batch > 0 {
            k[FREE] = 0;
        }
        if self.profile.live_target > 0 {
            if self.live.len() < self.profile.live_target {
                k[FREE] = 0;
                k[SEND] = 0;
            } else {
                k[ALLOC] = 0;
                k[ZEROED] = 0;
            }
        }
        if self.live.is_empty() {
            for i in [FREE, RESIZE, TOUCH, SEND] {
                k[i] = 0;
            }
        }
        if k.iter().all(|&x| x == 0) {
            k[ALLOC] = 1;
        }
        k
    }

    fn fresh_id(&mut self) -> BlockId {
        let id = BlockId(self.next_id);
        self.next_id += 1;
        id
    }

    fn draw_size<E: Entropy>(&self, e: &mut E, allow_zero: bool) -> Option<usize> {
        if allow_zero && e.below(64)? == 0 {
            return Some(0);
        }
        let size = match self.profile.sizes {
            Sizes::Fixed(table) => table[e.below(table.len() as u32)? as usize],
            Sizes::Bands(bands) => match e.weighted(&bands)? {
                0 => {
                    // Log-uniform: pick a doubling, then a size within it, so small classes of
                    // every size are exercised rather than the largest dominating.
                    let k = e.below(14)?;
                    let lo = 1usize << k;
                    let hi = ((1usize << (k + 1)) - 1).min(SMALL_MAX);
                    e.range(lo, hi)?
                }
                1 => e.range(10 * KIB, 100 * KIB)?,
                2 => e.range(100 * KIB, 4 * MIB)?,
                _ => e.range(4 * MIB, 64 * MIB)?,
            },
        };
        Some(size.clamp(1, self.profile.max_size))
    }

    fn draw_align<E: Entropy>(&self, e: &mut E) -> Option<usize> {
        let align = match e.weighted(&self.profile.aligns)? {
            0 => 1usize << e.below(4)?,
            1 => 16,
            2 => 32,
            3 => 64,
            4 => 1usize << e.range(7, 11)?,
            5 => 4096,
            6 => 1usize << e.range(13, 16)?,
            _ => 2 * MIB,
        };
        let cap = 1usize << (usize::BITS - 1 - self.profile.max_align.leading_zeros());
        Some(align.min(cap))
    }

    fn alloc_op<E: Entropy>(&mut self, e: &mut E, zeroed: bool) -> Option<Op> {
        let size = self.draw_size(e, self.zero.alloc)?;
        let align = self.draw_align(e)?;
        let variant = e.next_u32()?;
        let room = self.budget.saturating_sub(self.live_bytes);
        let size = if size <= room {
            size
        } else if !self.live.is_empty() {
            return self.free_op(e);
        } else {
            room.max(1)
        };
        let id = self.fresh_id();
        self.live.push(Entry { id, size, align });
        self.live_bytes += size;
        if self.batch_left > 0 {
            self.batch_left -= 1;
            if self.batch_left == 0 {
                self.free_left = self.live.len();
            }
        }
        Some(Op::Alloc {
            id,
            size,
            align,
            zeroed,
            variant,
        })
    }

    fn free_op<E: Entropy>(&mut self, e: &mut E) -> Option<Op> {
        let len = self.live.len();
        let (i, keep_order) = match self.profile.order {
            Order::Random => (e.below(len as u32)? as usize, false),
            Order::Lifo => (len - 1, false),
            Order::Fifo => (0, true),
        };
        let variant = e.next_u32()?;
        let entry = self.live.remove_at(i, keep_order);
        self.live_bytes -= entry.size;
        Some(Op::Free {
            id: entry.id,
            variant,
        })
    }

    fn draw_resize<E: Entropy>(&self, e: &mut E, old: usize) -> Option<usize> {
        Some(match self.profile.growth {
            Growth::Near => {
                if e.below(2)? == 0 {
                    let lo = (old / 2).max(1);
                    let hi = old.saturating_mul(2).clamp(lo, lo + u32::MAX as usize);
                    e.range(lo, hi)?
                } else {
                    self.draw_size(e, self.zero.resize)?
                }
            }
            Growth::Grow => {
                if e.below(5)? != 0 {
                    let percent = e.range(25, 100)?;
                    old.saturating_add((old.saturating_mul(percent) / 100).max(1))
                } else {
                    let lo = old / 4;
                    e.range(lo, old.min(lo + u32::MAX as usize))?
                }
            }
        })
    }

    fn resize_op<E: Entropy>(&mut self, e: &mut E) -> Option<Op> {
        let i = e.below(self.live.len() as u32)? as usize;
        let entry = self.live.order[i];
        let wanted = self.draw_resize(e, entry.size)?;
        let align = if e.below(8)? == 0 {
            self.draw_align(e)?
        } else {
            entry.align
        };
        let zeroed = e.below(4)? == 0;
        let variant = e.next_u32()?;
        let room = self.budget.saturating_sub(self.live_bytes) + entry.size;
        let floor = usize::from(!self.zero.resize);
        let size = wanted.min(room).min(self.profile.max_size).max(floor);
        self.live_bytes = self.live_bytes - entry.size + size;
        self.live.order[i] = Entry {
            size,
            align,
            ..entry
        };
        Some(Op::Resize {
            id: entry.id,
            size,
            align,
            zeroed,
            variant,
        })
    }

    fn touch_op<E: Entropy>(&mut self, e: &mut E) -> Option<Op> {
        let i = e.below(self.live.len() as u32)? as usize;
        Some(Op::Touch {
            id: self.live.order[i].id,
        })
    }

    fn send_op<E: Entropy>(&mut self, e: &mut E) -> Option<Op> {
        let x = self
            .exchange
            .expect("send has weight only with an exchange");
        let i = e.below(self.live.len() as u32)? as usize;
        let to = match x.target {
            Target::Partner(p) => p,
            Target::AnyOther => {
                let k = e.below(x.workers - 1)?;
                if k >= x.me { k + 1 } else { k }
            }
            Target::Own => x.me,
        };
        let entry = self.live.remove_at(i, false);
        self.live_bytes -= entry.size;
        self.last_sent = Some(entry);
        Some(Op::Send { id: entry.id, to })
    }

    fn recv_op<E: Entropy>(&mut self, e: &mut E) -> Option<Op> {
        let x = self
            .exchange
            .expect("recv has weight only with an exchange");
        Some(Op::Recv {
            from: e.below(x.workers)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entropy::Rng;

    const ANY_ZERO: ZeroSize = ZeroSize {
        alloc: true,
        resize: true,
    };
    const NO_ZERO: ZeroSize = ZeroSize {
        alloc: false,
        resize: false,
    };

    fn stream(profile: Profile, zero: ZeroSize, seed: u64, n: usize) -> Vec<Op> {
        let mut g = Generator::new(profile, zero, 1, profile.max_live_bytes, None);
        let mut r = Rng::new(seed);
        (0..n).map(|_| g.next_op(&mut r).unwrap()).collect()
    }

    /// Replays a stream against a set of live ids, checking that every operation names a live
    /// block and that the live-byte cap holds.
    fn check_consistent(profile: Profile, ops: &[Op]) -> usize {
        let mut live: HashMap<BlockId, usize> = HashMap::new();
        let mut bytes = 0usize;
        let mut peak = 0;
        for op in ops {
            match *op {
                Op::Alloc {
                    id, size, align, ..
                } => {
                    assert!(align.is_power_of_two() && size <= profile.max_size);
                    assert!(live.insert(id, size).is_none(), "id reused");
                    bytes += size;
                }
                Op::Free { id, .. } => bytes -= live.remove(&id).expect("free of a live block"),
                Op::Resize { id, size, .. } => {
                    let old = live.insert(id, size).expect("resize of a live block");
                    bytes = bytes - old + size;
                }
                Op::Touch { id } => assert!(live.contains_key(&id)),
                Op::Send { .. } | Op::Recv { .. } => panic!("no exchange configured"),
                Op::Migrate => {}
            }
            assert!(bytes <= profile.max_live_bytes, "{bytes} live");
            peak = peak.max(live.len());
        }
        peak
    }

    #[test]
    fn every_profile_names_only_live_blocks_and_respects_the_budget() {
        for profile in Profile::ALL {
            let ops = stream(profile, NO_ZERO, 3, 5000);
            check_consistent(profile, &ops);
            assert!(
                ops.iter().any(|o| matches!(o, Op::Free { .. })),
                "{}",
                profile.name
            );
        }
    }

    #[test]
    fn churn_holds_its_population_and_batches_drain() {
        let peak = check_consistent(Profile::CHURN, &stream(Profile::CHURN, NO_ZERO, 1, 20_000));
        assert_eq!(peak, Profile::CHURN.live_target);
        let p = Profile::LIFO_BATCHES;
        assert!(check_consistent(p, &stream(p, NO_ZERO, 2, 20_000)) <= p.batch);
    }

    #[test]
    fn zero_sizes_appear_only_when_allowed() {
        let zero = |ops: &[Op]| {
            ops.iter()
                .any(|o| matches!(o, Op::Alloc { size: 0, .. } | Op::Resize { size: 0, .. }))
        };
        assert!(!zero(&stream(Profile::MIXED, NO_ZERO, 5, 5000)));
        assert!(zero(&stream(Profile::MIXED, ANY_ZERO, 5, 5000)));
    }

    #[test]
    fn caps_hold() {
        let p = Profile::LARGE_GROWING.capped(4096, 64, 1 << 20);
        for op in stream(p, NO_ZERO, 9, 5000) {
            if let Op::Alloc { size, align, .. } | Op::Resize { size, align, .. } = op {
                assert!(size <= 4096 && align <= 64, "{op}");
            }
        }
    }

    #[test]
    fn sends_go_where_the_exchange_says() {
        let x = Exchange {
            workers: 4,
            me: 2,
            send: 300,
            recv: 300,
            target: Target::AnyOther,
            alloc_percent: 100,
            free_percent: 100,
        };
        let mut g = Generator::new(Profile::MIXED, NO_ZERO, 1, 1 << 30, Some(x));
        let mut r = Rng::new(4);
        let mut seen = [0; 4];
        for _ in 0..5000 {
            match g.next_op(&mut r).unwrap() {
                Op::Send { to, .. } => {
                    seen[to as usize] += 1;
                    // Half the sends bounce, which must put the block back.
                    if seen[to as usize] % 2 == 0 {
                        let before = g.live_blocks();
                        g.observe(&Effect::Kept);
                        assert_eq!(g.live_blocks(), before + 1);
                    }
                }
                Op::Recv { from } => assert!(from < 4),
                _ => {}
            }
        }
        assert_eq!(seen[2], 0, "never to itself");
        assert!(seen[0] > 0 && seen[1] > 0 && seen[3] > 0);
    }
}
