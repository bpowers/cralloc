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

/// One mailbox (or deque, for [`Handoff::Steal`]) per worker.
#[derive(Debug)]
pub struct Mailroom {
    boxes: Box<[Mutex<VecDeque<Live>>]>,
    handoff: Handoff,
}

impl Mailroom {
    /// Mailboxes for `workers` workers.
    pub fn new(workers: usize, handoff: Handoff) -> Self {
        Mailroom {
            boxes: (0..workers).map(|_| Mutex::new(VecDeque::new())).collect(),
            handoff,
        }
    }

    /// The hand-off pattern.
    pub fn handoff(&self) -> Handoff {
        self.handoff
    }

    fn lock(&self, i: usize) -> MutexGuard<'_, VecDeque<Live>> {
        self.boxes[i].lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Put `block` in worker `to`'s mailbox, or give it back if the box is full or absent.
    pub fn post(&self, to: usize, block: Live) -> Result<(), Live> {
        if to >= self.boxes.len() {
            return Err(block);
        }
        let mut b = self.lock(to);
        if b.len() >= MAILBOX_CAPACITY {
            return Err(block);
        }
        b.push_back(block);
        Ok(())
    }

    /// Up to `max` blocks for worker `me`. For [`Handoff::Steal`], its own newest blocks, or if
    /// it has none, the oldest of worker `from`'s; otherwise the oldest in its own mailbox.
    pub fn collect(&self, me: usize, from: usize, max: usize) -> Vec<Live> {
        let mut out = Vec::new();
        if me >= self.boxes.len() {
            return out;
        }
        if self.handoff == Handoff::Steal {
            let mut own = self.lock(me);
            while out.len() < max {
                match own.pop_back() {
                    Some(b) => out.push(b),
                    None => break,
                }
            }
            drop(own);
            let victim = from % self.boxes.len();
            if out.is_empty() && victim != me {
                let mut theirs = self.lock(victim);
                while out.len() < max {
                    match theirs.pop_front() {
                        Some(b) => out.push(b),
                        None => break,
                    }
                }
            }
        } else {
            let mut own = self.lock(me);
            while out.len() < max {
                match own.pop_front() {
                    Some(b) => out.push(b),
                    None => break,
                }
            }
        }
        out
    }

    /// Everything still in any mailbox.
    pub fn drain_all(&self) -> Vec<Live> {
        (0..self.boxes.len())
            .flat_map(|i| std::mem::take(&mut *self.lock(i)))
            .collect()
    }
}
