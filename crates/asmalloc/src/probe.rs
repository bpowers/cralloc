//! Slow-path counters for investigations, behind the `probe-counts` feature; not for merging.
//!
//! One relaxed add per event into a static table that a debugger or `/proc/PID/mem` reads by
//! its symbol, `ASMALLOC_PROBE_COUNTS`: `u64` counters indexed by
//! `(cpu * SMALL_COUNT + class ordinal) * KINDS + kind`, for CPUs below [`CPUS`].
#![allow(missing_docs)]

use crate::classes::{SMALL_COUNT, SmallClass};
use crate::front::Cpu;
use core::sync::atomic::{AtomicU64, Ordering::Relaxed};

/// What happened, one counter each.
#[derive(Clone, Copy)]
#[repr(usize)]
pub enum Kind {
    AllocSlow = 0,
    FreeSlow,
    /// Allocation: a magazine with objects from the CPU's own spares.
    AllocSpare,
    /// Allocation: a magazine with objects from the depot.
    AllocDepot,
    /// Allocation: an empty magazine filled from the bitmaps.
    AllocFill,
    /// Free (or a fill): an empty magazine from the CPU's own spares.
    EmptySpare,
    /// An empty magazine from the depot.
    EmptyDepot,
    /// An empty magazine newly made.
    EmptyMade,
    /// A displaced or unused magazine with objects kept as a spare.
    RetireFullSpare,
    /// ... sent to the depot.
    RetireFullDepot,
    /// An empty magazine kept as a spare.
    RetireEmptySpare,
    /// ... sent to the depot.
    RetireEmptyDepot,
    /// A spare kept past the class's fixed limit because its stock admitted it.
    StockAdmit,
    /// A magazine laundered back to the bitmaps.
    Launder,
    /// Allocation: the sum of the levels of the magazines installed.
    AllocInstalledLevel,
    /// Free: the sum of the levels of the full magazines displaced.
    FreeDisplacedLevel,
}

pub const KINDS: usize = 16;
pub const CPUS: usize = 32;

#[unsafe(no_mangle)]
#[used]
pub static ASMALLOC_PROBE_COUNTS: [AtomicU64; CPUS * SMALL_COUNT * KINDS] =
    [const { AtomicU64::new(0) }; CPUS * SMALL_COUNT * KINDS];

#[inline]
pub fn count(cpu: Option<Cpu>, class: SmallClass, kind: Kind) {
    add(cpu, class, kind, 1);
}

#[inline]
pub fn add(cpu: Option<Cpu>, class: SmallClass, kind: Kind, n: u64) {
    let cpu = cpu.map_or(CPUS - 1, |c| c.index().min(CPUS - 1));
    let at = (cpu * SMALL_COUNT + class.ordinal() % SMALL_COUNT) * KINDS + kind as usize;
    if let Some(c) = ASMALLOC_PROBE_COUNTS.get(at) {
        c.fetch_add(n, Relaxed);
    }
}

/// The direction of the last swap at a boundary for each CPU and class, for the centring
/// experiment (`probe-center`): 0 none, 1 allocation, 2 free.
#[cfg(feature = "probe-center")]
static LAST: [core::sync::atomic::AtomicU8; CPUS * SMALL_COUNT] =
    [const { core::sync::atomic::AtomicU8::new(0) }; CPUS * SMALL_COUNT];

/// Records a boundary swap in direction `dir` (1 allocation, 2 free) and says whether the
/// last one on this CPU and class went the other way: a swing that crosses both ends.
#[cfg(feature = "probe-center")]
pub fn reversed(cpu: Option<Cpu>, class: SmallClass, dir: u8) -> bool {
    let Some(cpu) = cpu else { return false };
    let at = cpu.index().min(CPUS - 1) * SMALL_COUNT + class.ordinal() % SMALL_COUNT;
    match LAST.get(at) {
        Some(last) => {
            let was = last.swap(dir, Relaxed);
            was != 0 && was != dir
        }
        None => false,
    }
}
