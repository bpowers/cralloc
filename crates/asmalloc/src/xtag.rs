//! EXPERIMENT (domain-affine back end): the cache domain that last pushed each magazine onto a
//! shared stack, in a side table by magazine number. A real design would keep it in the
//! magazine's record. Hints only: a stale or folded entry costs a wrong choice, nothing more.

use core::sync::atomic::AtomicU8;
use core::sync::atomic::Ordering::Relaxed;

static PUSHER: [AtomicU8; 1 << 20] = [const { AtomicU8::new(0) }; 1 << 20];

/// Records that `domain` pushed magazine `id` onto a shared stack.
#[inline]
pub fn pushed(id: u32, domain: usize) {
    PUSHER[id as usize % PUSHER.len()].store((domain % 255) as u8 + 1, Relaxed);
}

/// Whether magazine `id` was last pushed onto a shared stack by a domain other than `domain`.
#[inline]
pub fn from_other(id: u32, domain: usize) -> bool {
    let p = PUSHER[id as usize % PUSHER.len()].load(Relaxed);
    p != 0 && p as usize - 1 != domain % 255
}
