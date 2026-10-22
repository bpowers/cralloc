//! Siblings handed out a stride apart.
//!
//! Objects of one class allocated one after another are the allocator's best evidence of
//! siblings: instances of one type made in a loop. When different threads then write them,
//! siblings that share a cache line take it from each other on every write. tokio allocates
//! one 40-byte run-queue header per worker at start-up; packed side by side, they cost the
//! spawn jobs 14% at four workers and 8% at eight (`docs/research/spawn-gap.md`).
//!
//! So a magazine whose fill does not continue a run of fills of its class
//! ([`Filled::opens_run`](super::Filled)), the class's first fill of an interval, hands its
//! first [`SIBLINGS`] strides of objects out in `stride` passes of [`SIBLINGS`] each, the
//! objects of a pass a [`stride`] apart, the fewest slots at which two objects of the class are
//! sure to share no line; the passes fill each other's gaps, and the rest of the magazine
//! follows in the fill's order. So 64 objects allocated one after another lie on lines of their
//! own (192 objects are reordered for the 40-byte class, 512 for the 8-byte one). No slot is
//! skipped, and the bitmaps and frontiers are as the fill left them: only the order within the
//! magazine changes.
//!
//! The run rule comes from what the order costs. Objects handed out apart are touched apart: a
//! tree walked in the order it was built takes more lines and loses the hardware prefetchers'
//! stream. A program that builds a structure fills again and again, so only the first magazine
//! of its run is affected; strided on every fill, a tree's build and walk lost 47% and a JSON
//! parse 6%. Servers whose small classes circulate keep a strided magazine's order in their
//! working set; with one binary and the stride switched at run time, neither 64 strides nor
//! whole magazines moved the RPC servers measured (`docs/research/spawn-gap.md`, "The rule,
//! built"). The runtimes' sibling sets are small: tokio's headers need as many as its workers.
//!
//! Only classes smaller than a line, where several objects share each line. Classes of one to
//! two lines (80 to 112 bytes) put neighbours on a shared boundary line too, but no measured
//! case asked for them.

use crate::lined::LINE;
use core::sync::atomic::AtomicPtr;
use core::sync::atomic::Ordering::Relaxed;

/// How many objects one after another a fill that opens a run keeps on lines of their own: the
/// length of each of its `stride` passes.
pub(crate) const SIBLINGS: usize = 64;

/// How many slots apart two objects of `size` bytes must be to be sure of sharing no line, or
/// `None` for a class of a line or more. A size that divides a line is the alignment of its
/// objects, which packs whole objects into lines, so `LINE / size` slots apart are on
/// different lines. Any other size has objects that straddle a boundary, and two of them are
/// sure to share no line, wherever they start, only with a line's bytes between them:
/// `(stride - 1) * size >= LINE`. 24 bytes: 4; 40, 48 and 56: 3.
pub(crate) const fn stride(size: usize) -> Option<usize> {
    if size == 0 || size >= LINE {
        None
    } else if LINE.is_multiple_of(size) {
        Some(LINE / size)
    } else {
        Some(LINE.div_ceil(size) + 1)
    }
}

/// The most objects [`spread`] reorders: [`SIBLINGS`] strides of the smallest class's.
const MOST: usize = SIBLINGS * LINE / 8;

/// Where each of the objects a magazine hands out first goes when they are handed out a stride
/// apart.
///
/// The `p`-th object the fill would have handed out, counting from zero, is at position `p`.
/// Strided, the positions are handed out in passes: pass `r` takes `r`, `r + stride`,
/// `r + 2 * stride` and so on, in that order, and the passes run from the last, `r = stride -
/// 1`, to the first, so that the first pass ends a stride below the first position after the
/// strided ones. With `len = rows * stride + longer`, the passes below `longer` take `rows + 1`
/// positions and the others `rows`.
#[derive(Clone, Copy)]
struct Strided {
    stride: usize,
    rows: usize,
    longer: usize,
}

impl Strided {
    fn new(len: usize, stride: usize) -> Strided {
        Strided {
            stride,
            rows: len / stride,
            longer: len % stride,
        }
    }

    /// How many objects are handed out before the one at position `p`: what the passes after
    /// its own take, plus its place in its pass.
    fn place(self, p: usize) -> usize {
        let (row, pass) = (p / self.stride, p % self.stride);
        let later = self.stride - 1 - pass;
        later * self.rows + self.longer.saturating_sub(pass + 1) + row
    }
}

/// Reorders the objects of a magazine this thread holds, which a fill wrote from the top
/// (`objects[len - 1]` is popped first), so that the first [`SIBLINGS`] strides of them are
/// handed out `stride` positions apart (see [`Strided`]), and the rest as they were. Once at
/// least two strides are reordered, no two objects handed out one after another among those,
/// or between the last of them and the next, are fewer than `stride` positions apart. A fill
/// writes ascending addresses, so those are distinct slots at least `stride` apart and share no
/// line, unless the fill moved past another filler and then took slots lower in the region
/// (`tree::claim_next`): two positions a stride apart may then be neighbours. Fewer than two
/// strides cannot be kept apart, and are left alone.
///
/// In place, one cycle of the permutation at a time, each object moved once: the slow path runs
/// on whatever stack the program called it from. A bit per position, 64 bytes, says which
/// already hold their own object.
#[cold]
#[inline(never)]
pub(super) fn spread(objects: &[AtomicPtr<u8>], stride: usize) {
    let len = objects.len().min(SIBLINGS.saturating_mul(stride));
    if stride < 2 || len < 2 * stride || len > MOST {
        return;
    }
    let Some(first) = objects.get(objects.len() - len..) else {
        return;
    };
    let order = Strided::new(len, stride);
    // Position `p` is the `p`-th object handed out. The remainder keeps the index visibly in
    // bounds; `len - 1 - p` is below `len` for every position.
    let at = |p: usize| &first[(len - 1 - p) % len];
    let mut placed = [0u64; MOST.div_ceil(64)];
    let words = placed.len();
    for start in 0..len {
        if placed[(start / 64) % words] & 1 << (start % 64) != 0 {
            continue;
        }
        let mut carried = at(start).load(Relaxed);
        let mut from = start;
        // A cycle of the permutation is at most `len` long and ends where it began.
        for _ in 0..len {
            let to = order.place(from);
            let slot = at(to);
            let displaced = slot.load(Relaxed);
            slot.store(carried, Relaxed);
            placed[(to / 64) % words] |= 1 << (to % 64);
            if to == start {
                break;
            }
            carried = displaced;
            from = to;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::classes::{MagazineSize, SmallClass};
    use core::ptr;
    use std::vec::Vec;

    #[test]
    fn the_stride_is_the_fewest_slots_that_never_share_a_line() {
        let strides: Vec<(usize, Option<usize>)> =
            (1..=8).map(|n| (8 * n, stride(8 * n))).collect();
        assert_eq!(
            strides,
            [
                (8, Some(8)),
                (16, Some(4)),
                (24, Some(4)),
                (32, Some(2)),
                (40, Some(3)),
                (48, Some(3)),
                (56, Some(3)),
                (64, None)
            ]
        );
        // Objects `apart` slots apart share a line for some object, the first starting at any
        // multiple of `align` within a line.
        let shares = |size: usize, apart: usize, align: usize| {
            (0..LINE).step_by(align).any(|start| {
                (0..LINE).any(|i| {
                    let (a, b) = (start + i * size, start + (i + apart) * size);
                    (a + size - 1) / LINE == b / LINE
                })
            })
        };
        for class in SmallClass::all() {
            let size = class.size();
            let Some(s) = stride(size) else {
                assert!(size >= LINE);
                continue;
            };
            if LINE.is_multiple_of(size) {
                // Such a class's objects are aligned to their size: none straddles a line, and
                // the stride is the fewest that works.
                assert!(!shares(size, s, size), "{size}: {s}");
                assert!(shares(size, s - 1, size), "{size}: {s} is not the fewest");
            } else {
                // Wherever the objects start, and the fewest that holds so. Where the class's
                // alignment rules out some starts, fewer may do (48 and 56 bytes: 2).
                assert!(!shares(size, s, 1), "{size}: {s}");
                assert!(shares(size, s - 1, 1), "{size}: {s} is not the fewest");
            }
            assert!(SIBLINGS * s <= MOST, "{size}: {s}");
            assert!(
                class.magazine().capacity() > SIBLINGS * s,
                "{size}: a full magazine holds the siblings and more"
            );
        }
    }

    /// A magazine of `len` slots holding the numbers `0..len` as its fill writes them: the
    /// lowest on top.
    fn magazine(len: usize) -> Vec<AtomicPtr<u8>> {
        (0..len)
            .rev()
            .map(|n| AtomicPtr::new(ptr::without_provenance_mut(n)))
            .collect()
    }

    /// The numbers in the order the magazine hands them out: from the top.
    fn handed_out(objects: &[AtomicPtr<u8>]) -> Vec<usize> {
        objects
            .iter()
            .rev()
            .map(|o| o.load(Relaxed).addr())
            .collect()
    }

    /// Every length a magazine can have and every stride up to a line's worth of the smallest
    /// class: the reorder hands out each object once, the first `SIBLINGS` strides of them in
    /// passes `stride` apart, last pass first, and the rest as they were; objects handed out
    /// one after another are at least `stride` apart up to the first of the rest.
    #[test]
    fn a_spread_magazine_hands_out_every_object_once_its_siblings_a_stride_apart() {
        let most = MagazineSize::MAX.capacity();
        let lens: Vec<usize> = if cfg!(miri) {
            [0, 1, 5, 15, 16, 17, 23, 254, most].to_vec()
        } else {
            (0..=most).collect()
        };
        for s in 2..=8 {
            for &len in &lens {
                let objects = magazine(len);
                spread(&objects, s);
                let got = handed_out(&objects);
                let strided = len.min(SIBLINGS * s);
                if strided < 2 * s {
                    assert_eq!(got, (0..len).collect::<Vec<_>>(), "{s} {len}: untouched");
                    continue;
                }
                let expected: Vec<usize> = (0..s)
                    .rev()
                    .flat_map(|r| (r..strided).step_by(s))
                    .chain(strided..len)
                    .collect();
                assert_eq!(got, expected, "{s} {len}");
                for pair in got[..len.min(strided + 1)].windows(2) {
                    assert!(pair[0].abs_diff(pair[1]) >= s, "{s} {len}: {pair:?}");
                }
            }
        }
    }

    #[test]
    fn a_stride_too_small_to_separate_or_too_large_to_hold_is_left_alone() {
        for (len, s) in [(40, 1), (40, 0), (2000, 9)] {
            let objects = magazine(len);
            spread(&objects, s);
            assert_eq!(handed_out(&objects), (0..len).collect::<Vec<_>>());
        }
    }
}
