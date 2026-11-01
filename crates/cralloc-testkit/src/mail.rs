//! Hand-off between workers, so blocks are freed on a different thread from the one that
//! allocated them.

use std::collections::VecDeque;
use std::sync::{Mutex, MutexGuard, PoisonError};

use crate::model::Live;

/// How blocks move between workers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Handoff {
    /// Blocks stay with the worker that allocated them.
    None,
    /// Single producer, single consumer: even workers allocate and send to the next odd worker,
    /// which frees.
    Pairs,
    /// Every worker sends to random others and frees what it receives.
    AllToAll,
    /// Work stealing: each worker pushes blocks onto its own deque and pops them back newest
    /// first; idle workers steal the oldest from someone else's.
    Steal,
}

/// Blocks a mailbox holds before a send bounces.
pub const MAILBOX_CAPACITY: usize = 1024;

#[derive(Debug, Default)]
struct Mailbox {
    blocks: VecDeque<Live>,
    /// Requested bytes of the blocks held.
    bytes: usize,
}

impl Mailbox {
    fn pop(&mut self, newest_first: bool) -> Option<Live> {
        let b = if newest_first {
            self.blocks.pop_back()
        } else {
            self.blocks.pop_front()
        }?;
        self.bytes -= b.held.size;
        Some(b)
    }
}

/// One mailbox (or deque, for [`Handoff::Steal`]) per worker.
#[derive(Debug)]
pub struct Mailroom {
    boxes: Box<[Mutex<Mailbox>]>,
    handoff: Handoff,
    byte_capacity: usize,
}

impl Mailroom {
    /// Mailboxes for `workers` workers, each holding at most [`MAILBOX_CAPACITY`] blocks and,
    /// beyond its first block, `byte_capacity` requested bytes. Blocks in a mailbox are live
    /// memory that no worker's budget counts, so the cap keeps the run's memory bounded.
    pub fn new(workers: usize, handoff: Handoff, byte_capacity: usize) -> Self {
        Mailroom {
            boxes: (0..workers).map(|_| Mutex::default()).collect(),
            handoff,
            byte_capacity,
        }
    }

    fn lock(&self, i: usize) -> MutexGuard<'_, Mailbox> {
        self.boxes[i].lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Put `block` in worker `to`'s mailbox, or give it back if the box is full or absent.
    pub fn post(&self, to: usize, block: Live) -> Result<(), Live> {
        if to >= self.boxes.len() {
            return Err(block);
        }
        let mut b = self.lock(to);
        let full = b.blocks.len() >= MAILBOX_CAPACITY
            || (!b.blocks.is_empty() && b.bytes + block.held.size > self.byte_capacity);
        if full {
            return Err(block);
        }
        b.bytes += block.held.size;
        b.blocks.push_back(block);
        Ok(())
    }

    /// Blocks for worker `me`: at least one if any is available, and then more until `limit`
    /// blocks or `limit_bytes` requested bytes are taken. For [`Handoff::Steal`], its own newest
    /// blocks, or if it has none, the oldest of worker `from`'s; otherwise the oldest in its own
    /// mailbox.
    pub fn collect(&self, me: usize, from: usize, limit: usize, limit_bytes: usize) -> Vec<Live> {
        let mut out = Vec::new();
        if me >= self.boxes.len() {
            return out;
        }
        let take = |mailbox: &mut Mailbox, out: &mut Vec<Live>, newest_first: bool| {
            let mut bytes = 0;
            while out.len() < limit && (out.is_empty() || bytes < limit_bytes) {
                let Some(b) = mailbox.pop(newest_first) else {
                    break;
                };
                bytes += b.held.size;
                out.push(b);
            }
        };
        let steal = self.handoff == Handoff::Steal;
        take(&mut self.lock(me), &mut out, steal);
        let victim = from % self.boxes.len();
        if steal && out.is_empty() && victim != me {
            take(&mut self.lock(victim), &mut out, false);
        }
        out
    }

    /// In a child process after `fork`, everything in the mailboxes whose lock is free. A box
    /// locked at the fork belonged to a thread that does not exist here, so it is skipped
    /// rather than waited for.
    pub fn take_after_fork(&self) -> Vec<Live> {
        let mut out = Vec::new();
        for b in self.boxes.iter() {
            let mut guard = match b.try_lock() {
                Ok(g) => g,
                Err(std::sync::TryLockError::Poisoned(p)) => p.into_inner(),
                Err(std::sync::TryLockError::WouldBlock) => continue,
            };
            out.extend(std::mem::take(&mut *guard).blocks);
        }
        out
    }

    /// Everything still in any mailbox.
    pub fn drain_all(&self) -> Vec<Live> {
        (0..self.boxes.len())
            .flat_map(|i| std::mem::take(&mut *self.lock(i)).blocks)
            .collect()
    }
}
