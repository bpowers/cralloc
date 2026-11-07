//! Whole heaps on simulated kernels at their real size (`os::sim::Kernel4k` and the rest): every
//! interface's sizes and alignments, blocks moved and resized in place over staggers of many pages,
//! and a heap whose count of retained memory ends where it can return all it counted.

use super::*;
use crate::os::sim::{Kernel4k, Kernel16k, Kernel64k, Shape};

const KIB: usize = 1 << 10;
const MIB: usize = 1 << 20;

/// What the heap serves in a test: small classes, each side of the page term's first class on each
/// kernel, a huge page's size and past it on each. Few enough classes for a heap of few regions.
const SIZES: [usize; 11] = [
    16,
    KIB,
    8 * KIB,
    40 * KIB,
    70 * KIB,
    300 * KIB,
    MIB + 1,
    3 * MIB,
    9 * MIB,
    33 * MIB,
    70 * MIB,
];

const ALIGNS: [usize; 6] = [8, 16, 64, 128, 4 * KIB, 128 * KIB];

/// A small generator, for a repeatable mix.
struct Rng(u64);

impl Rng {
    fn below(&mut self, n: usize) -> usize {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 % n as u64) as usize
    }
}

/// Writes a byte that depends on `seed` at the start of the block, every 4 KiB through it and at its
/// end.
unsafe fn mark(p: *mut u8, len: usize, seed: u8) {
    let mut at = 0;
    while at < len {
        // SAFETY: inside the block the caller holds.
        unsafe { p.add(at).write(seed ^ (at >> 12) as u8) };
        at += 4 * KIB;
    }
    // SAFETY: as above; a block's length is not zero.
    unsafe { p.add(len - 1).write(seed ^ ((len - 1) >> 12) as u8) };
}

/// Whether the first `len` bytes of a block `mark` wrote with `seed` over `whole` bytes still say so.
unsafe fn marked(p: *const u8, len: usize, whole: usize, seed: u8) -> bool {
    let holds = |at: usize| {
        // SAFETY: inside the block the caller holds.
        at >= len || unsafe { p.add(at).read() } == seed ^ (at >> 12) as u8
    };
    (0..whole).step_by(4 * KIB).all(holds) && holds(whole - 1)
}

/// A random mix over `heap`, with everything checked as it goes, down to nothing live; then what
/// `collect` leaves is nothing retained, and zeroed memory reads as zero.
fn serve<S: Shape>(heap: SimHeap<S>, seed: u64) {
    let mut rng = Rng(seed);
    let mut live: Vec<(*mut u8, Layout, u8)> = Vec::new();
    let mut held = 0usize;
    // A heap of few regions runs out of them for classes it has not used yet: a null that costs the
    // test its coverage if it is common.
    let mut refused = 0;
    for op in 0..600u32 {
        let choice = rng.below(8);
        if (choice < 4 && held < 256 * MIB) || live.is_empty() {
            let size = SIZES[rng.below(SIZES.len())];
            let align = ALIGNS[rng.below(ALIGNS.len())];
            let new = layout(size, align);
            let zeroed = rng.below(4) == 0;
            // SAFETY: valid layouts with sizes that are not zero.
            let p = unsafe {
                if zeroed {
                    heap.alloc_zeroed(new)
                } else {
                    heap.alloc(new)
                }
            };
            if p.is_null() {
                refused += 1;
                continue;
            }
            assert!(p.addr().is_multiple_of(align), "{p:?} for {new:?}");
            let usable = heap.usable_size(p).unwrap();
            assert!(usable >= size, "{usable} usable of {size}");
            if zeroed {
                // SAFETY: inside the block.
                let bytes = unsafe { core::slice::from_raw_parts(p, size) };
                assert!(bytes.iter().step_by(61).all(|&b| b == 0), "{new:?} zeroed");
            }
            // SAFETY: the caller may use all it was told it may.
            unsafe { p.add(usable - 1).write(1) };
            let tag = op as u8 | 1;
            // SAFETY: inside the block.
            unsafe { mark(p, size, tag) };
            held += size;
            live.push((p, new, tag));
        } else if choice < 6 {
            let i = rng.below(live.len());
            let (p, old, tag) = live.swap_remove(i);
            // SAFETY: the block is live and written by `mark` over its size.
            assert!(unsafe { marked(p, old.size(), old.size(), tag) }, "{old:?}");
            // SAFETY: allocated with this layout.
            unsafe { heap.dealloc(p, old) };
            held -= old.size();
        } else {
            let i = rng.below(live.len());
            let (p, old, tag) = live[i];
            let size = SIZES[rng.below(SIZES.len())];
            // SAFETY: the block is live, allocated with `old`, and the size is not zero.
            let q = unsafe { heap.realloc(p, old, size) };
            if q.is_null() {
                refused += 1;
                continue;
            }
            let kept = old.size().min(size);
            // SAFETY: what was kept was written by `mark`.
            assert!(
                unsafe { marked(q, kept, old.size(), tag) },
                "{old:?} to {size}"
            );
            assert!(q.addr().is_multiple_of(old.align()), "{q:?} for {old:?}");
            let new = layout(size, old.align());
            // SAFETY: inside the block.
            unsafe { mark(q, size, tag) };
            held = held - old.size() + size;
            live[i] = (q, new, tag);
        }
    }
    assert!(refused < 150, "{refused} of 600 calls refused");
    for (p, old, tag) in live {
        // SAFETY: as above.
        assert!(unsafe { marked(p, old.size(), old.size(), tag) });
        // SAFETY: allocated with this layout.
        unsafe { heap.dealloc(p, old) };
    }
    heap.collect();
    let back = heap.heap.back().unwrap();
    // A large slot's charge is exact, so nothing of it is left. A small class keeps the free slots
    // of the page its watermark is in, which no purge may take.
    assert_eq!(back.retained_large(), 0, "a large slot's charge all went back");
    let page = S::GEOMETRY.page_size();
    assert!(back.retained() <= 16 * page, "{} retained", back.retained());
    for &size in &SIZES {
        let new = layout(size, 64);
        // SAFETY: a valid layout.
        let p = unsafe { heap.alloc_zeroed(new) };
        if p.is_null() {
            continue;
        }
        // SAFETY: inside the block.
        let bytes = unsafe { core::slice::from_raw_parts(p, size) };
        assert!(bytes.iter().all(|&b| b == 0), "{size} zeroed");
        // SAFETY: allocated with this layout.
        unsafe { heap.dealloc(p, new) };
    }
}

#[test]
#[cfg_attr(miri, ignore)]
fn a_heap_serves_every_size_with_4_kib_pages() {
    serve(SimHeap::<Kernel4k>::new(), 0x1357_9bdf_0246_8ace);
}

#[test]
#[cfg_attr(miri, ignore)]
fn a_heap_serves_every_size_with_16_kib_pages() {
    serve(SimHeap::<Kernel16k>::new(), 0x2468_ace0_1357_9bdf);
}

#[test]
#[cfg_attr(miri, ignore)]
fn a_heap_serves_every_size_with_64_kib_pages() {
    serve(SimHeap::<Kernel64k>::new(), 0x0f1e_2d3c_4b5a_6978);
}

/// The same on regions smaller than the kernel's huge page, which a limit on address space leaves:
/// a block too large for a region takes a run of them.
#[test]
#[cfg_attr(miri, ignore)]
fn a_heap_serves_every_size_on_regions_under_a_huge_page() {
    let os = SimOs::<Kernel16k>::new();
    // Regions of 1 GiB down to 16 MiB are refused.
    let faults = Faults {
        pass: 0,
        fail: 7,
        partial: false,
    };
    os.set_faults(Call::Reserve, faults);
    let heap = SimHeap::with_os(os);
    serve(heap, 0x7766_5544_3322_1100);
}

/// A kernel's real sizes are the same rules' on every kernel: a block with headroom on 16 KiB pages
/// is charged what it wrote, as it is on 4 KiB, not its room.
#[test]
#[cfg_attr(miri, ignore)]
fn a_grown_block_on_a_large_page_is_charged_what_it_wrote() {
    let heap = SimHeap::<Kernel16k>::new();
    let page = Kernel16k::GEOMETRY.page_size();
    let small = layout(MIB, 64);
    // SAFETY: valid layouts; each block is written within its size and freed once with its layout.
    unsafe {
        let p = heap.alloc(small);
        fill(p, small.size(), 1);
        let q = heap.realloc(p, small, 2 * MIB);
        assert_ne!(p, q, "outgrew its slot");
        fill(q, 2 * MIB, 2);
        let back = heap.heap.back().unwrap();
        let before = back.retained_large();
        heap.dealloc(q, layout(2 * MIB, 64));
        let charged = back.retained_large() - before;
        // Its block's pages, and one for where the stagger leaves its end.
        assert!(charged <= 2 * MIB + page, "charged {charged}");
        assert!(charged >= 2 * MIB, "charged {charged}");
    }
}
