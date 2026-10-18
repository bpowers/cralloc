//! The model: one worker's operations, each checked against what the interface promised.
//!
//! A [`Model`] holds the blocks one worker owns, each with the seed of the pattern the model
//! wrote into it. For every operation it checks:
//!
//! - a new block is aligned as requested and as the interface promised, has at least the
//!   requested usable size, and overlaps no live block of any worker (the shared [`Shadow`]);
//! - memory promised zero is zero;
//! - every byte the model wrote into a block is still there whenever the block is touched,
//!   freed, resized, sent or received, so a write by the allocator into live memory is caught
//!   at the block's next touch;
//! - a resize carries over the bytes it promised;
//! - the usable size the interface reports for a live block does not change;
//! - the allocator serves every request inside the live-byte budget.
//!
//! The model writes and checks the usable extent, not just the request, so an allocator that
//! over-reports its usable size is caught too; but never more than [`MAX_SLACK`] bytes past the
//! request, since an allocator may legitimately report a huge block (a large slot, a run of
//! regions) and filling it would commit memory the test never asked for.
//!
//! A model is driven one operation at a time ([`Model::execute`]), which is what a
//! deterministic scheduler over simulated threads needs as well as the thread drivers here.

use std::collections::HashMap;

use crate::failure::{Failure, FailureKind};
use crate::mail::Mailroom;
use crate::op::{BlockId, Op};
use crate::pattern::{self, Contents};
use crate::shadow::Shadow;
use crate::subject::{Grant, Held, Refused, Request, Subject};

/// Blocks taken from the mailroom by one receive, at most.
const MAX_RECEIVE: usize = 8;
/// Bytes past which a receive takes no further block, so a receiver near its budget does not
/// take eight large blocks at once.
const MAX_RECEIVE_BYTES: usize = 1 << 20;

/// Bytes past the request that the model writes and checks at most.
pub const MAX_SLACK: usize = 64 << 10;

/// The bytes of a block the model touches: its usable extent, capped at [`MAX_SLACK`] past the
/// request.
fn reach(size: usize, usable: usize) -> usize {
    usable.min(size.saturating_add(MAX_SLACK))
}

/// A live block and what the model wrote into it.
#[derive(Debug)]
pub struct Live {
    /// The block's name.
    pub id: BlockId,
    /// The block as the subject described it.
    pub held: Held,
    /// Seed of the pattern over the block's reach.
    pub seed: u64,
}

/// What an operation changed that the operation's source needs to know.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    /// Nothing beyond what the operation says.
    None,
    /// The allocation produced no block (a zero-size request that returned null).
    Gone(BlockId),
    /// The block was not sent (the receiver's mailbox was full); the sender still owns it.
    Kept,
    /// These blocks arrived from other workers.
    Adopted(Vec<Adopted>),
    /// The stream asked to continue on a new thread.
    Migrate,
}

/// A block received from another worker.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Adopted {
    /// Its name.
    pub id: BlockId,
    /// Its requested size.
    pub size: usize,
    /// Its alignment.
    pub align: usize,
}

/// Counters for a run.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    /// Operations executed.
    pub ops: usize,
    /// Allocations without a zero-fill promise.
    pub allocs: usize,
    /// Allocations with one.
    pub zeroed_allocs: usize,
    /// Zero-size allocations that returned null.
    pub null_allocs: usize,
    /// Frees, including the final drain.
    pub frees: usize,
    /// Resizes.
    pub resizes: usize,
    /// Touches.
    pub touches: usize,
    /// Blocks handed to another worker.
    pub sends: usize,
    /// Blocks received from another worker.
    pub receives: usize,
    /// Thread hops.
    pub migrations: usize,
    /// Forks.
    pub forks: usize,
    /// Highest bytes requested and live at once in one worker.
    pub peak_live_bytes: usize,
    /// Highest blocks live at once in one worker.
    pub peak_live_blocks: usize,
}

impl Stats {
    /// Add another worker's counters.
    pub fn merge(&mut self, o: &Stats) {
        self.ops += o.ops;
        self.allocs += o.allocs;
        self.zeroed_allocs += o.zeroed_allocs;
        self.null_allocs += o.null_allocs;
        self.frees += o.frees;
        self.resizes += o.resizes;
        self.touches += o.touches;
        self.sends += o.sends;
        self.receives += o.receives;
        self.migrations += o.migrations;
        self.forks += o.forks;
        self.peak_live_bytes = self.peak_live_bytes.max(o.peak_live_bytes);
        self.peak_live_blocks = self.peak_live_blocks.max(o.peak_live_blocks);
    }

    /// Blocks allocated (excluding nulls for zero-size requests).
    pub fn blocks_allocated(&self) -> usize {
        self.allocs + self.zeroed_allocs
    }
}

/// One worker's view of the allocator.
pub struct Model<'e, S: ?Sized> {
    subject: &'e S,
    shadow: &'e Shadow,
    mail: Option<&'e Mailroom>,
    worker: usize,
    live: HashMap<BlockId, Live>,
    live_bytes: usize,
    serial: u64,
    op_index: usize,
    stats: Stats,
}

impl<'e, S: Subject + ?Sized> Model<'e, S> {
    /// A model for worker `worker`, sharing `shadow` and `mail` with the others.
    pub fn new(
        subject: &'e S,
        shadow: &'e Shadow,
        mail: Option<&'e Mailroom>,
        worker: usize,
    ) -> Self {
        Model {
            subject,
            shadow,
            mail,
            worker,
            live: HashMap::new(),
            live_bytes: 0,
            serial: (worker as u64) << 40,
            op_index: 0,
            stats: Stats::default(),
        }
    }

    /// The counters so far.
    pub fn stats(&self) -> Stats {
        self.stats
    }

    /// The subject under test.
    pub fn subject(&self) -> &'e S {
        self.subject
    }

    /// The shared index.
    pub fn shadow(&self) -> &'e Shadow {
        self.shadow
    }

    /// The shared mailboxes.
    pub fn mail(&self) -> Option<&'e Mailroom> {
        self.mail
    }

    /// Count a fork this worker made.
    pub fn note_fork(&mut self) {
        self.stats.forks += 1;
    }

    /// Execute one operation and check its results.
    pub fn execute(&mut self, op: &Op) -> Result<Effect, Failure> {
        let result = match *op {
            Op::Alloc {
                id,
                size,
                align,
                zeroed,
                variant,
            } => self.alloc(
                op,
                id,
                Request {
                    size,
                    align,
                    zeroed,
                    variant,
                },
            ),
            Op::Free { id, variant } => self.free(op, id, variant),
            Op::Resize {
                id,
                size,
                align,
                zeroed,
                variant,
            } => self.resize(
                op,
                id,
                Request {
                    size,
                    align,
                    zeroed,
                    variant,
                },
            ),
            Op::Touch { id } => self.touch(op, id),
            Op::Send { id, to } => self.send(op, id, to),
            Op::Recv { from } => self.receive(op, from),
            Op::Migrate => {
                self.stats.migrations += 1;
                Ok(Effect::Migrate)
            }
        };
        self.op_index += 1;
        self.stats.ops += 1;
        result
    }

    /// Free every block this worker owns, in name order, checking each.
    pub fn drain(&mut self) -> Result<(), Failure> {
        let mut ids: Vec<BlockId> = self.live.keys().copied().collect();
        ids.sort_unstable();
        for id in ids {
            let op = Op::Free {
                id,
                variant: id.0 as u32,
            };
            self.free(&op, id, id.0 as u32)?;
        }
        Ok(())
    }

    /// Take ownership of a block another worker held, checking it.
    pub fn adopt(&mut self, block: Live) -> Result<Adopted, Failure> {
        self.verify(None, &block, "when received")?;
        Ok(self.insert(block))
    }

    /// Take ownership of a block that is not in this model's shadow yet (a block inherited
    /// across `fork`, when the child builds a fresh index), claiming its interval.
    pub fn adopt_unclaimed(&mut self, block: Live) -> Result<Adopted, Failure> {
        self.claim(None, block.id, &block.held)?;
        self.adopt(block)
    }

    /// Give up every block, unchecked and still claimed in the shadow.
    pub fn take_blocks(&mut self) -> Vec<Live> {
        self.live_bytes = 0;
        self.live.drain().map(|(_, b)| b).collect()
    }

    fn fail(&self, kind: FailureKind, op: Option<&Op>, detail: String) -> Failure {
        Failure {
            kind,
            subject: self.subject.name(),
            worker: self.worker,
            op_index: self.op_index,
            op: op.copied(),
            detail,
            origin: None,
            trace: None,
        }
    }

    fn insert(&mut self, block: Live) -> Adopted {
        let adopted = Adopted {
            id: block.id,
            size: block.held.size,
            align: block.held.align,
        };
        self.live_bytes += block.held.size;
        self.live.insert(block.id, block);
        self.stats.peak_live_bytes = self.stats.peak_live_bytes.max(self.live_bytes);
        self.stats.peak_live_blocks = self.stats.peak_live_blocks.max(self.live.len());
        adopted
    }

    fn remove(&mut self, id: BlockId) -> Option<Live> {
        let block = self.live.remove(&id)?;
        self.live_bytes -= block.held.size;
        Some(block)
    }

    /// Check a block's contents and reported usable size.
    fn verify(&self, op: Option<&Op>, block: &Live, when: &str) -> Result<(), Failure> {
        let h = &block.held;
        let len = reach(h.size, h.usable);
        // SAFETY: the block is live and owned by this worker, and the model wrote every byte of
        // its reach.
        let r =
            unsafe { pattern::check(h.ptr.as_ptr(), len, 0, len, Contents::Pattern(block.seed)) };
        if let Err(m) = r {
            return Err(self.fail(
                FailureKind::Corrupted,
                op,
                format!(
                    "block {} at {:#x} ({} bytes requested, {} usable) has byte {} = {:#04x}, \
                     expected {:#04x}, {when}",
                    block.id,
                    h.addr(),
                    h.size,
                    h.usable,
                    m.offset,
                    m.found,
                    m.expected
                ),
            ));
        }
        // SAFETY: the block is live.
        if let Some(now) = unsafe { self.subject.usable_size(*h) }
            && now != h.usable
        {
            return Err(self.fail(
                FailureKind::UsableSizeChanged,
                op,
                format!(
                    "block {} at {:#x} had usable size {} and now reports {now}, {when}",
                    block.id,
                    h.addr(),
                    h.usable
                ),
            ));
        }
        Ok(())
    }

    /// Check a fresh block's alignment and usable size against the request (`size`,
    /// `required_align`) and the interface's promise.
    fn check_grant(
        &self,
        op: &Op,
        what: &str,
        g: &Grant,
        size: usize,
        required_align: usize,
    ) -> Result<(), Failure> {
        let addr = g.ptr.addr().get();
        if !g.align.is_power_of_two() || g.align < required_align || !addr.is_multiple_of(g.align) {
            return Err(self.fail(
                FailureKind::Misaligned,
                Some(op),
                format!(
                    "{what} returned {addr:#x} for {size} bytes; alignment required {required_align}, \
                     promised {}",
                    g.align
                ),
            ));
        }
        if g.usable < size {
            return Err(self.fail(
                FailureKind::ShortBlock,
                Some(op),
                format!(
                    "{what} returned {addr:#x} with {} usable bytes for a request of {size}",
                    g.usable
                ),
            ));
        }
        Ok(())
    }

    /// Bytes of a block recorded in the shadow: its usable extent, or one byte for a distinct
    /// zero-size block, which is never read or written.
    fn extent(&self, h: &Held) -> usize {
        if h.usable == 0 && h.size == 0 && self.subject.zero_size().distinct {
            1
        } else {
            h.usable
        }
    }

    /// Record a block's extent in the shadow, or report what it overlaps.
    fn claim(&self, op: Option<&Op>, id: BlockId, h: &Held) -> Result<(), Failure> {
        let extent = self.extent(h);
        if extent == 0 {
            return Ok(());
        }
        let start = h.addr();
        let Some(end) = start.checked_add(extent) else {
            return Err(self.fail(
                FailureKind::Overlap,
                op,
                format!("block {id} at {start:#x} of {extent} bytes wraps the address space"),
            ));
        };
        self.shadow.claim(start, end, id).map_err(|other| {
            let whose = if self.live.contains_key(&other.owner) {
                "this worker's"
            } else {
                "another worker's"
            };
            self.fail(
                FailureKind::Overlap,
                op,
                format!(
                    "block {id} at [{start:#x}, {end:#x}) overlaps {whose} live block {} at \
                     [{:#x}, {:#x})",
                    other.owner, other.start, other.end
                ),
            )
        })
    }

    fn unclaim(&self, block: &Live) {
        let h = &block.held;
        let extent = self.extent(h);
        if extent > 0 {
            self.shadow.release(h.addr(), h.addr() + extent, block.id);
        }
    }

    /// Check that `[from, to)` of a fresh block is zero.
    fn check_zero(
        &self,
        op: &Op,
        what: &str,
        g: &Grant,
        size: usize,
        from: usize,
        to: usize,
    ) -> Result<(), Failure> {
        let len = reach(size, g.usable);
        let to = to.min(len);
        if from >= to {
            return Ok(());
        }
        // SAFETY: the interface promised these bytes zero, which initializes them, and they lie
        // inside the usable extent.
        let r = unsafe { pattern::check(g.ptr.as_ptr(), len, from, to, Contents::Zero) };
        r.map_err(|m| {
            self.fail(
                FailureKind::NotZeroed,
                Some(op),
                format!(
                    "{what} returned {:#x} promising bytes {from}..{to} zero, but byte {} = {:#04x}",
                    g.ptr.addr().get(),
                    m.offset,
                    m.found
                ),
            )
        })
    }

    /// Write a fresh pattern over a block and return its seed.
    fn fill(&mut self, h: &Held) -> u64 {
        self.serial += 1;
        let len = reach(h.size, h.usable);
        let seed = pattern::seed(h.addr(), len, self.serial);
        // SAFETY: the block is live and owned by this worker, with `usable` writable bytes, of
        // which `len` is a prefix.
        unsafe { pattern::fill(h.ptr.as_ptr(), len, seed) };
        seed
    }

    fn alloc(&mut self, op: &Op, id: BlockId, req: Request) -> Result<Effect, Failure> {
        if self.live.contains_key(&id) {
            return Ok(Effect::None);
        }
        let what = if req.zeroed { "zeroed alloc" } else { "alloc" };
        let zero_ok = self.subject.zero_size().alloc;
        let req = Request {
            size: if zero_ok { req.size } else { req.size.max(1) },
            ..req
        };
        // SAFETY: the request is well formed: the generator and the script parser only build
        // power-of-two alignments and sizes far below `isize::MAX`, and zero only when allowed.
        let grant = match unsafe { self.subject.alloc(req) } {
            Ok(Some(g)) => g,
            Ok(None) if req.size == 0 => {
                self.stats.null_allocs += 1;
                return Ok(Effect::Gone(id));
            }
            Ok(None) | Err(Refused) => {
                return Err(self.fail(
                    FailureKind::Refused,
                    Some(op),
                    format!(
                        "{what} refused {} bytes aligned to {} with {} bytes live in this worker",
                        req.size, req.align, self.live_bytes
                    ),
                ));
            }
        };
        self.check_grant(op, what, &grant, req.size, req.align)?;
        let held = Held::new(&grant, req.size);
        self.claim(Some(op), id, &held)?;
        self.check_zero(op, what, &grant, req.size, 0, grant.zeroed)?;
        if grant.zeroed > 0 {
            self.stats.zeroed_allocs += 1;
        } else {
            self.stats.allocs += 1;
        }
        let seed = self.fill(&held);
        self.insert(Live { id, held, seed });
        Ok(Effect::None)
    }

    fn free(&mut self, op: &Op, id: BlockId, variant: u32) -> Result<Effect, Failure> {
        let Some(block) = self.remove(id) else {
            return Ok(Effect::None);
        };
        self.verify(Some(op), &block, "when freed")?;
        self.unclaim(&block);
        // SAFETY: the block is live, exactly as the subject described it, and no longer in the
        // shadow, so a reuse of its memory is not mistaken for an overlap.
        unsafe { self.subject.free(block.held, variant) };
        self.stats.frees += 1;
        Ok(Effect::None)
    }

    fn resize(&mut self, op: &Op, id: BlockId, req: Request) -> Result<Effect, Failure> {
        let Some(old) = self.remove(id) else {
            return Ok(Effect::None);
        };
        self.verify(Some(op), &old, "before resize")?;
        let req = Request {
            size: if self.subject.zero_size().resize {
                req.size
            } else {
                req.size.max(1)
            },
            ..req
        };
        self.unclaim(&old);
        // SAFETY: the block is live, exactly as the subject described it; the request is well
        // formed as for `alloc`.
        let resized = match unsafe { self.subject.resize(old.held, req) } {
            Ok(r) => r,
            Err(Refused) => {
                let detail = format!(
                    "resize of block {id} from {} to {} bytes refused with {} bytes live in this \
                     worker",
                    old.held.size, req.size, self.live_bytes
                );
                self.claim(Some(op), id, &old.held)?;
                self.insert(old);
                return Err(self.fail(FailureKind::Refused, Some(op), detail));
            }
        };
        let g = resized.grant;
        // The interface's promise is the whole requirement for a resize: `realloc` keeps the
        // old alignment and C's `realloc` only promises the natural one.
        self.check_grant(op, "resize", &g, req.size, 1)?;
        let held = Held::new(&g, req.size);
        self.claim(Some(op), id, &held)?;
        let old_len = reach(old.held.size, old.held.usable);
        let keep = resized.preserved.min(old_len).min(g.usable);
        // SAFETY: the interface promised to carry over `keep` bytes, which initializes them.
        let r = unsafe {
            pattern::check(
                g.ptr.as_ptr(),
                old_len,
                0,
                keep,
                Contents::Pattern(old.seed),
            )
        };
        if let Err(m) = r {
            return Err(self.fail(
                FailureKind::LostContents,
                Some(op),
                format!(
                    "resize of block {id} from {:#x} ({} bytes) to {} bytes returned {:#x} \
                     promising {keep} bytes carried over, but byte {} = {:#04x}, expected {:#04x}",
                    old.held.addr(),
                    old.held.size,
                    req.size,
                    g.ptr.addr().get(),
                    m.offset,
                    m.found,
                    m.expected
                ),
            ));
        }
        if let Some(from) = resized.zeroed_from {
            self.check_zero(op, "zeroed resize", &g, req.size, from, g.usable)?;
        }
        let seed = self.fill(&held);
        self.insert(Live { id, held, seed });
        self.stats.resizes += 1;
        Ok(Effect::None)
    }

    fn touch(&mut self, op: &Op, id: BlockId) -> Result<Effect, Failure> {
        let Some(block) = self.remove(id) else {
            return Ok(Effect::None);
        };
        self.verify(Some(op), &block, "when touched")?;
        let seed = self.fill(&block.held);
        self.insert(Live { seed, ..block });
        self.stats.touches += 1;
        Ok(Effect::None)
    }

    fn send(&mut self, op: &Op, id: BlockId, to: u32) -> Result<Effect, Failure> {
        let Some(mail) = self.mail else {
            return Ok(Effect::Kept);
        };
        let Some(block) = self.remove(id) else {
            return Ok(Effect::None);
        };
        self.verify(Some(op), &block, "when sent")?;
        match mail.post(to as usize, block) {
            Ok(()) => {
                self.stats.sends += 1;
                Ok(Effect::None)
            }
            Err(block) => {
                self.insert(block);
                Ok(Effect::Kept)
            }
        }
    }

    fn receive(&mut self, op: &Op, from: u32) -> Result<Effect, Failure> {
        let Some(mail) = self.mail else {
            return Ok(Effect::None);
        };
        let mut adopted = Vec::new();
        for block in mail.collect(self.worker, from as usize, MAX_RECEIVE, MAX_RECEIVE_BYTES) {
            self.verify(Some(op), &block, "when received")?;
            adopted.push(self.insert(block));
        }
        self.stats.receives += adopted.len();
        Ok(Effect::Adopted(adopted))
    }
}
