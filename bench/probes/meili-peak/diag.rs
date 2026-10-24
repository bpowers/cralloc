//! Scratch counters for the account of Meilisearch's peak (bench/probes/meili-peak/peak_edit.py).
//! Not product code. The sampler (memstat_peak.rs) reads them from the same process.
#![allow(missing_docs, dead_code)]
use core::sync::atomic::{AtomicU8, AtomicU32, AtomicU64, Ordering::Relaxed};

pub const N: usize = 1280;

/// Bytes returned to the OS, by the rule that returned them. The walk's rules count what the
/// clean bits say was resident; the slot owner's rules count the range they returned.
pub const RET_HOLD: usize = 0;
pub const RET_PASS: usize = 1;
pub const RET_GROWTH: usize = 2;
pub const RET_TRIM: usize = 3;
pub const RET_FREE_ADVISED: usize = 4;
pub const RET_MOVE: usize = 5;
pub const RET_SHRINK: usize = 6;
/// Bytes carved past a watermark: small classes (whole slots) and large (the size asked for).
pub const CARVE_SMALL: usize = 7;
pub const CARVE_LARGE: usize = 8;
/// Pages a widening of a held large block (`region::cover`) found clean: growth in place.
pub const COVER: usize = 9;
/// Growth payments (`pay_growth`): the debt taken, what was owed over the cap, the step's
/// goal, what stayed owed, what was dropped because the walk found nothing, and growth
/// dropped because retained memory was under the cap (here and in `growth_due`).
pub const DEBT_TAKEN: usize = 10;
pub const DEBT_OWED: usize = 11;
pub const DEBT_GOAL: usize = 12;
pub const DEBT_CARRIED: usize = 13;
pub const DEBT_DRY: usize = 14;
pub const DEBT_UNDER_CAP: usize = 15;
/// Claims of free large slots: how many, the bytes asked for, the bytes of their pages the
/// clean bits said were not resident, and the bytes `mincore` said were not resident.
pub const LARGE_CLAIMS: usize = 16;
pub const LARGE_CLAIM_BYTES: usize = 17;
pub const LARGE_CLAIM_CLEAN: usize = 18;
pub const LARGE_CLAIM_NONRES: usize = 19;
/// Frees of large slots left resident: how many, what the retained count was charged, and
/// what `mincore` said was resident.
pub const LARGE_FREES: usize = 20;
pub const LARGE_FREE_CHARGE: usize = 21;
pub const LARGE_FREE_RES: usize = 22;
/// Bytes of purged pages that small fills took back.
pub const SMALL_REVIVE: usize = 23;
/// Set when the heap is inspected: the retained counts, the hold's fine count, the debt.
pub const RETAINED_SMALL: usize = 24;
pub const RETAINED_LARGE: usize = 25;
pub const HOLD_LOOKS: usize = 26;
pub const HOLD_FINE: usize = 27;
pub const DEBT_NOW: usize = 28;
pub const LARGE_CARVES: usize = 29;
pub const KEPT: usize = 30;
pub const KEPT_REFUSED: usize = 31;
pub const MOVES: usize = 32;
pub const MOVE_BYTES: usize = 33;
/// Growth payments made, and payments that found the count under the cap.
pub const PAYMENTS: usize = 34;
pub const PAY_UNDER_CAP: usize = 35;
/// The prototype's own counters (bench/probes/meili-peak/proto_edit.py).
pub const PROTO_0: usize = 40;

/// Rules a region's last purge came from.
pub const R_NONE: u8 = 0;
pub const R_HOLD: u8 = 1;
pub const R_PASS: u8 = 2;
pub const R_GROWTH: u8 = 3;
pub const R_TRIM: u8 = 4;
pub const R_SELF: u8 = 5;
pub const RULES: usize = 6;
pub const AGES: usize = 8;
/// Bytes large claims will fault (`mincore`), by the rule that last purged the slot's region
/// and how long before, in fine steps (about 2 ms): [rule][age bucket], `age_bucket`. Pages
/// the clean bits did not mark (a previous tenant's size never written) are rule `R_NONE`.
pub const LARGE_FAULT_BY: usize = 64;
/// Bytes of purged pages small fills took back, by rule.
pub const SMALL_REVIVE_BY: usize = 112;
/// Per large class index (1..=112): bytes asked for by claims, bytes they will fault, bytes
/// carved, bytes charged at free, bytes resident at free, bytes the walk returned (any rule).
pub const CLASS_CLAIM: usize = 128;
pub const CLASS_FAULT: usize = 256;
pub const CLASS_CARVE: usize = 384;
pub const CLASS_FREE_CHARGE: usize = 512;
pub const CLASS_FREE_RES: usize = 640;
pub const CLASS_RET: usize = 768;
/// Per small class: bytes carved and bytes revived.
pub const CLASS_SMALL_CARVE: usize = 896;
pub const CLASS_SMALL_REVIVE: usize = 1024;

pub const REGIONS: usize = 4096;

#[unsafe(no_mangle)]
pub static ASMALLOC_DIAG: [AtomicU64; N] = [const { AtomicU64::new(0) }; N];
/// The rule of each region's last purge, and the fine count then.
pub static REGION_RULE: [AtomicU8; REGIONS] = [const { AtomicU8::new(0) }; REGIONS];
pub static REGION_FINE: [AtomicU32; REGIONS] = [const { AtomicU32::new(0) }; REGIONS];
/// Each large region's hold age in fine steps when last inspected; `u32::MAX` for no stamp.
pub static REGION_AGE: [AtomicU32; REGIONS] = [const { AtomicU32::new(u32::MAX) }; REGIONS];
/// Each region's class index, written at every carve, and where the heap's regions start, so
/// that the sampler's counting wrapper can tell the class of any block.
pub static REGION_CLS: [AtomicU8; REGIONS] = [const { AtomicU8::new(0) }; REGIONS];
pub static HEAP_BASE: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
pub static REGION_SHIFT: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

/// The class index of the region holding `addr`, zero if unknown.
pub fn class_of(addr: usize) -> u8 {
    let base = HEAP_BASE.load(Relaxed);
    if base == 0 || addr < base {
        return 0;
    }
    let index = (addr - base) >> REGION_SHIFT.load(Relaxed);
    if index < REGIONS { REGION_CLS[index].load(Relaxed) } else { 0 }
}

#[inline(always)]
pub fn add(i: usize, v: usize) {
    if v != 0 && i < N {
        ASMALLOC_DIAG[i].fetch_add(v as u64, Relaxed);
    }
}

#[inline(always)]
pub fn set(i: usize, v: usize) {
    if i < N {
        ASMALLOC_DIAG[i].store(v as u64, Relaxed);
    }
}

pub fn rule(region: usize, rule: u8, fine: u32) {
    if region < REGIONS {
        REGION_RULE[region].store(rule, Relaxed);
        REGION_FINE[region].store(fine, Relaxed);
    }
}

/// Age buckets in fine steps: under 32 (64 ms), 64 (125 ms, the hold), 128, 256 (0.5 s), 512
/// (1 s), 1024 (2 s), 4096 (8 s), and older.
pub const fn age_bucket(age: u32) -> usize {
    match age {
        0..32 => 0,
        32..64 => 1,
        64..128 => 2,
        128..256 => 3,
        256..512 => 4,
        512..1024 => 5,
        1024..4096 => 6,
        _ => 7,
    }
}
