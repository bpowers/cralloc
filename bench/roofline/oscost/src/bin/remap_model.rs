//! The soak's `realloc` workload (`crates/asmalloc-preload/tests/programs/soak.c`) replayed
//! against a model of asmalloc's large-object rules, to compare ways of moving a block without
//! changing the allocator: growth chains to a random top and back down, contents written and
//! checked as the soak does (every byte up to 1 MiB, 64 bytes per page up to 64 MiB, 64 bytes
//! per MiB above).
//!
//! The model keeps the allocator's rules that matter here: four classes per doubling, a move on
//! growth goes to a slot with four times the room, a shrink below a quarter of the slot moves,
//! a shrink in place returns its tail, freed large slots stay resident up to 8 MiB in total.
//! Blocks under 64 KiB are plain `malloc` blocks in every variant.
//!
//! usage: remap_model glibc|copy|sparse|remap [max MiB] [chains] [least remap KiB]
//!   glibc   the C library's realloc, for reference
//!   copy    2 MiB pieces, source pages returned behind each (what asmalloc does now)
//!   sparse  the same, skipping source pages that are all zero
//!   remap   one mremap of the largest piece that is one mapping, sparse copy for the rest
//! Prints reallocs per second and the process's peak resident memory.

#[path = "../remap_sys.rs"]
mod sys;
use std::time::Instant;
use sys::*;

extern "C" {
    fn malloc(n: usize) -> *mut std::ffi::c_void;
    fn realloc(p: *mut std::ffi::c_void, n: usize) -> *mut std::ffi::c_void;
    fn free(p: *mut std::ffi::c_void);
}

const REGION: usize = 4 * GB;
const LARGE: usize = 64 * KB;
const PIECE: usize = HUGE;
const RETAINED_CAP: usize = 8 * MB;
const HEADROOM: usize = 4;
const STRIPE: usize = 64;

#[derive(Clone, Copy, PartialEq)]
enum Way {
    Glibc,
    Copy,
    Sparse,
    Remap,
}

fn mix64(mut x: u64) -> u64 {
    x ^= x >> 33;
    x = x.wrapping_mul(0xff51afd7ed558ccd);
    x ^= x >> 33;
    x = x.wrapping_mul(0xc4ceb9fe1a85ec53);
    x ^ (x >> 33)
}

/// The class sizes from `LARGE` up to a region: four per doubling.
fn classes() -> Vec<usize> {
    let mut v = vec![];
    let mut base = LARGE;
    while base < REGION {
        for q in 4..8 {
            v.push(base / 4 * q);
        }
        base *= 2;
    }
    v.push(REGION);
    v
}

/// One slot per class is enough: a chain holds one block at a time.
#[derive(Clone, Copy, Default)]
struct Slot {
    /// Pages of an earlier block are still resident here.
    dirty: bool,
    /// The byte range that was moved in by remapping, if any.
    moved_in: Option<(usize, usize)>,
}

struct Model {
    way: Way,
    least: usize,
    base: *mut u8,
    classes: Vec<usize>,
    slots: Vec<Slot>,
    retained: usize,
    moves: u64,
    remaps: u64,
    copied: u64,
}

/// A block: a `malloc` block when small, else slot `class` of the model.
#[derive(Clone, Copy)]
struct Block {
    p: *mut u8,
    class: Option<usize>,
    size: usize,
}

impl Model {
    fn new(way: Way, least: usize) -> Model {
        let classes = classes();
        let base = reserve(classes.len() * REGION, REGION);
        for r in 0..classes.len() {
            prepare_region(unsafe { base.add(r * REGION) }, REGION, REGION);
            // One written page, so the region's mapping has its bookkeeping before any move.
            unsafe {
                base.add(r * REGION).write_volatile(1);
                madvise(base.add(r * REGION), PAGE, MADV_DONTNEED);
            }
        }
        let slots = vec![Slot::default(); classes.len()];
        Model { way, least, base, classes, slots, retained: 0, moves: 0, remaps: 0, copied: 0 }
    }

    fn class_of(&self, size: usize) -> usize {
        self.classes.iter().position(|&c| c >= size).expect("a size within a region")
    }

    /// The second slot of the class's region where there is one, so that the address has the
    /// alignment a slot of that class usually has, not a region's.
    fn at(&self, class: usize) -> *mut u8 {
        let size = self.classes[class];
        let index = if 2 * size <= REGION { 1 } else { 0 };
        unsafe { self.base.add(class * REGION + index * size) }
    }

    fn purge(&mut self, class: usize) {
        let (p, size) = (self.at(class), self.classes[class]);
        unsafe { madvise(p, size, MADV_DONTNEED) };
        if self.slots[class].dirty {
            self.retained -= size.min(self.retained);
        }
        self.slots[class].dirty = false;
    }

    /// Gives a slot up. A moved-in range gets a fresh mapping; otherwise the slot stays
    /// resident if the cap allows, as a freed large slot does.
    fn release(&mut self, class: usize, used: usize) {
        if let Some((a, b)) = self.slots[class].moved_in.take() {
            self.purge(class);
            assert_eq!(heal(unsafe { self.at(class).add(a) }, b - a), 0);
        } else if self.retained + self.classes[class] > RETAINED_CAP {
            self.purge(class);
        } else if used > 0 {
            self.slots[class].dirty = true;
            self.retained += self.classes[class];
        }
    }

    fn alloc(&mut self, size: usize, room: usize) -> Block {
        if room < LARGE {
            return Block { p: unsafe { malloc(size) }.cast(), class: None, size };
        }
        let class = self.class_of(room);
        if self.slots[class].dirty {
            // A copy may write over an earlier block's pages; the other ways need zeros.
            if self.way == Way::Copy {
                self.retained -= self.classes[class].min(self.retained);
                self.slots[class].dirty = false;
            } else {
                self.purge(class);
            }
        }
        Block { p: self.at(class), class: Some(class), size }
    }

    fn free(&mut self, b: Block) {
        match b.class {
            None => unsafe { free(b.p.cast()) },
            Some(class) => self.release(class, b.size),
        }
    }

    fn resize(&mut self, b: Block, new: usize) -> Block {
        if self.way == Way::Glibc {
            return Block { p: unsafe { realloc(b.p.cast(), new) }.cast(), class: None, size: new };
        }
        let Some(class) = b.class else {
            if new < LARGE {
                return Block { p: unsafe { realloc(b.p.cast(), new) }.cast(), class: None, size: new };
            }
            let to = self.alloc(new, new);
            unsafe { std::ptr::copy_nonoverlapping(b.p, to.p, b.size.min(new)) };
            self.free(b);
            return to;
        };
        let room = self.classes[class];
        if new >= LARGE && new <= room && (new >= b.size || new >= room / HEADROOM) {
            // In place; a shrink returns the whole pages past its new size.
            let (from, to) = (new.next_multiple_of(PAGE), b.size.next_multiple_of(PAGE));
            if from < to {
                unsafe { madvise(b.p.add(from), to - from, MADV_DONTNEED) };
            }
            return Block { size: new, ..b };
        }
        let roomy = if new > room && new * HEADROOM <= REGION { new * HEADROOM } else { new };
        let to = self.alloc(new, roomy);
        let kept = b.size.min(new);
        self.moves += 1;
        match to.class {
            None => {
                unsafe { std::ptr::copy_nonoverlapping(b.p, to.p, kept) };
                self.free(b);
            }
            Some(dest) => self.move_large(b, class, to.p, dest, kept),
        }
        to
    }

    fn move_large(&mut self, b: Block, class: usize, to: *mut u8, dest: usize, kept: usize) {
        let from = b.p;
        if self.way == Way::Copy {
            self.copied += kept as u64;
            if kept <= PIECE {
                unsafe { std::ptr::copy_nonoverlapping(from, to, kept) };
                return self.release(class, b.size);
            }
            let mut done = 0;
            while done < kept {
                let step = PIECE.min(kept - done);
                unsafe {
                    std::ptr::copy_nonoverlapping(from.add(done), to.add(done), step);
                    madvise(from.add(done), step, MADV_DONTNEED);
                }
                done += step;
            }
            return self.purge(class);
        }
        let pages = kept.next_multiple_of(PAGE);
        let mut moved = (0, 0);
        if self.way == Way::Remap {
            let limit = pages.min(self.classes[dest] - PAGE);
            let (a, c) = self.slots[class].moved_in.unwrap_or((0, 0));
            let pieces = [(0, a.min(limit)), (a.min(limit), c.min(limit)), (c.min(limit), limit)];
            let (start, end) = pieces.into_iter().max_by_key(|&(s, e)| e - s).unwrap();
            if end - start >= self.least
                && probe(unsafe { from.add(start) }, end - start) == ENOMEM
                && unsafe { madvise(from.add(start), PAGE, MADV_COLD) } == 0
                && remap(unsafe { from.add(start) }, unsafe { to.add(start) }, end - start) == 0
            {
                moved = (start, end);
                self.remaps += 1;
                self.slots[dest].moved_in = Some(moved);
            }
        }
        for (start, end) in [(0, moved.0), (moved.1, pages)] {
            let mut done = start;
            while done < end {
                let step = PIECE.min(end - done);
                unsafe {
                    self.copied += (copy_sparse(from.add(done), to.add(done), step) * PAGE) as u64;
                    if end - start > PIECE {
                        madvise(from.add(done), step, MADV_DONTNEED);
                    }
                }
                done += step;
            }
        }
        if kept <= PIECE && self.slots[class].moved_in.is_none() {
            return self.release(class, b.size);
        }
        self.purge(class);
        self.release(class, 0);
    }
}

fn stride_of(n: usize) -> usize {
    if n <= MB {
        0
    } else if n > 64 * MB {
        MB
    } else {
        PAGE
    }
}

/// The soak's covered ranges of an `n`-byte pattern that lie in `from..lim`.
fn covered(n: usize, from: usize, lim: usize, mut body: impl FnMut(usize, usize)) {
    let stride = stride_of(n);
    let lim = lim.min(n);
    if stride == 0 {
        if from < lim {
            body(from, lim);
        }
        return;
    }
    let mut o = from - from % stride;
    while o < lim {
        let (a, b) = (o.max(from), (o + STRIPE).min(lim));
        if a < b {
            body(a, b);
        }
        o += stride;
    }
    let (a, b) = ((n - STRIPE).max(from), lim);
    if a < b {
        body(a, b);
    }
}

fn fill(p: *mut u8, n: usize, from: usize, seed: u64) {
    let write = |a: usize, b: usize| {
        for i in a..b {
            unsafe { p.add(i).write(mix64(seed ^ (i as u64 >> 3)).to_le_bytes()[i & 7]) };
        }
    };
    covered(n, from, n, write);
    if stride_of(n) != 0 {
        write(n - STRIPE, n);
    }
}

/// Checks the pattern over the bytes that both an `n`-byte pattern covers and a block of
/// `lim` bytes holds.
fn verify(p: *mut u8, n: usize, lim: usize, seed: u64) {
    covered(n, 0, lim, |a, b| {
        for i in a..b {
            let want = mix64(seed ^ (i as u64 >> 3)).to_le_bytes()[i & 7];
            assert_eq!(unsafe { p.add(i).read() }, want, "offset {i} of {n}");
        }
    });
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let way = match args.first().map(String::as_str) {
        Some("glibc") => Way::Glibc,
        Some("copy") => Way::Copy,
        Some("sparse") => Way::Sparse,
        Some("remap") => Way::Remap,
        _ => panic!("usage: remap_model glibc|copy|sparse|remap [max MiB] [chains] [least remap KiB]"),
    };
    let arg = |i: usize, default: usize| args.get(i).and_then(|v| v.parse().ok()).unwrap_or(default);
    let (max, chains, least) = (arg(1, 256) * MB, arg(2, 40), arg(3, 1024) * KB);
    pin(std::env::var("RF_CPU").ok().and_then(|s| s.parse().ok()).unwrap_or(13));
    let mut model = Model::new(way, least);
    let mut rng = Rng(0x2545f4914f6cdd1d);
    let mut reallocs = 0u64;
    let start = Instant::now();
    for _ in 0..chains {
        let first = 1 + rng.below(64);
        let seed = rng.next();
        let mut b = model.alloc(first, first);
        // `n` is how many bytes the pattern covers: all of the block.
        fill(b.p, b.size, 0, seed);
        let top = 1 + rng.below(max);
        let mut step = |model: &mut Model, b: Block, to: usize| -> Block {
            verify(b.p, b.size, b.size, seed);
            let old = b.size;
            let next = model.resize(b, to);
            verify(next.p, old, old.min(to), seed);
            // The pattern of the new size from where the old one ended, as the soak refills.
            fill(next.p, to, old.min(to), seed);
            // A sparse pattern's stripes move when the stride does; rewrite them all then.
            if stride_of(to) != stride_of(old) {
                fill(next.p, to, 0, seed);
            }
            reallocs += 1;
            next
        };
        while b.size < top {
            let to = b.size + b.size * (5 + rng.below(96)) / 100 + 1 + rng.below(16);
            b = step(&mut model, b, to.min(max));
            if b.size >= max {
                break;
            }
        }
        while b.size > 1 {
            let to = b.size / (2 + rng.below(3)) + 1;
            b = step(&mut model, b, to);
        }
        verify(b.p, b.size, b.size, seed);
        if way == Way::Glibc {
            unsafe { free(b.p.cast()) };
        } else {
            model.free(b);
        }
    }
    let secs = start.elapsed().as_secs_f64();
    println!(
        "{:>6}: {reallocs} reallocs in {secs:.2} s = {:.0}/s; {} moves of large blocks, {} with a remap, {} MiB copied; peak resident {} MiB; mappings at the end {}",
        args[0],
        reallocs as f64 / secs,
        model.moves,
        model.remaps,
        model.copied / MB as u64,
        proc_kb("status", "VmHWM:") / 1024,
        maps().len()
    );
}
