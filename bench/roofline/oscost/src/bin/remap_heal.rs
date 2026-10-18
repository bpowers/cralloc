//! What `mremap` does to a heap laid out like asmalloc's when a large block is moved with
//! `MREMAP_MAYMOVE | MREMAP_FIXED | MREMAP_DONTUNMAP`, and whether a fresh `mmap(MAP_FIXED)`
//! over the destination afterwards ("healing") lets it merge with its neighbours again.
//! Each test prints what it saw; nothing here is timed. Results and the kernel source they were
//! checked against are in `docs/research/zen5-roofline.md`, section 7.
//!
//! usage: remap_heal [basic|ingredients|cycles|fork|probe|mapcount|hole|mlock|thp|cgroup]...
//! With no argument, every test but `cgroup` (which wants a small memory cap) and `mapcount`
//! (which makes a million mappings).

#[path = "../remap_sys.rs"]
mod sys;
use sys::*;

const REGION: usize = 4 * GB;
const REGIONS: usize = 8;
/// How much of each region the tests make writable.
const COMMITTED: usize = 2 * GB;

struct Heap {
    base: *mut u8,
}

impl Heap {
    /// A reservation with `classes.len()` regions prepared, one per class size.
    fn new(classes: &[usize]) -> Heap {
        let base = reserve(REGIONS * REGION, REGION);
        for (r, _) in classes.iter().enumerate() {
            prepare_region(unsafe { base.add(r * REGION) }, REGION, COMMITTED);
        }
        Heap { base }
    }

    fn slot(&self, region: usize, class: usize, index: usize) -> *mut u8 {
        assert!((index + 1) * class <= COMMITTED);
        unsafe { self.base.add(region * REGION + index * class) }
    }

    fn count(&self) -> usize {
        count_in(self.base, REGIONS * REGION)
    }

    fn show(&self, title: &str) {
        show(title, self.base, &maps_in(self.base, REGIONS * REGION));
    }

    fn drop(self) {
        unsafe { munmap(self.base, REGIONS * REGION) };
    }
}

/// Writes a recognisable word at the start of each page.
fn fill(p: *mut u8, len: usize, seed: u64) {
    for page in 0..len / PAGE {
        unsafe { (p.add(page * PAGE) as *mut u64).write_volatile(seed + page as u64) };
    }
}

fn holds(p: *mut u8, len: usize, seed: u64) -> bool {
    (0..len / PAGE).all(|page| unsafe { (p.add(page * PAGE) as *const u64).read_volatile() } == seed + page as u64)
}

fn is_zero(p: *mut u8, len: usize) -> bool {
    let bytes = unsafe { std::slice::from_raw_parts(p, len) };
    bytes.iter().all(|&b| b == 0)
}

fn basic() {
    println!("== basic: one move, then heal");
    let (small, big) = (3 * MB, 8 * MB);
    let heap = Heap::new(&[small, big]);
    let baseline = heap.count();
    let src = heap.slot(0, small, 5);
    let dst = heap.slot(1, big, 3);
    // Neighbours on both sides are in use, and the destination holds an earlier owner's bytes.
    for i in [4, 6] {
        fill(heap.slot(0, small, i), small, 100 * i as u64);
    }
    for i in [2, 4] {
        fill(heap.slot(1, big, i), big, 1000 * i as u64);
    }
    fill(src, small, 7);
    touch(dst, big, 1, 0xEE);
    println!("  mappings in the heap before: {baseline} (after faults: {})", heap.count());
    heap.show("before");
    let rss = rss_kb();
    let e = remap(src, dst, small);
    println!(
        "  mremap: errno {e}; mappings {} (+{}); resident {} -> {} MiB",
        heap.count(),
        heap.count() as isize - baseline as isize,
        rss / 1024,
        rss_kb() / 1024
    );
    heap.show("after the move");
    println!("  destination holds the block: {}", holds(dst, small, 7));
    let tail = unsafe { std::slice::from_raw_parts(dst.add(small), big - small) };
    println!("  destination past the block still holds the earlier owner's bytes: {}", tail.iter().all(|&b| b == 0xEE));
    println!(
        "  source reads as zero: {}; neighbours intact: {}",
        is_zero(src, small),
        holds(heap.slot(0, small, 4), small, 400) && holds(heap.slot(0, small, 6), small, 600)
    );
    let e = heal(dst, small);
    println!(
        "  heal of the moved range: errno {e}; mappings {} (baseline {baseline}); reads zero: {}",
        heap.count(),
        is_zero(dst, small)
    );
    println!("  the rest of the destination slot is untouched by the heal: {}", tail.iter().all(|&b| b == 0xEE));
    heap.drop();
}

fn ingredients() {
    println!("== ingredients: what blocks the merge");
    let class = 4 * MB;
    let heap = Heap::new(&[class, class]);
    for i in 0..40 {
        touch(heap.slot(1, class, i), class, PAGE, 1);
    }
    let baseline = heap.count();
    let mut slot = 1;
    let mut moved = |heap: &Heap| -> *mut u8 {
        let src = heap.slot(0, class, slot);
        let dst = heap.slot(1, class, 2 * slot);
        slot += 1;
        fill(src, class, 9);
        assert_eq!(remap(src, dst, class), 0);
        dst
    };
    let report = |heap: &Heap, what: &str| {
        println!("  {what}: {} extra mappings", heap.count() as isize - baseline as isize);
    };
    let dst = moved(&heap);
    report(&heap, "after a move (no heal)");
    unsafe {
        assert_eq!(madvise(dst, class, MADV_DONTNEED), 0);
        report(&heap, "MADV_DONTNEED on the moved range");
        assert_eq!(madvise(dst, class, MADV_NOHUGEPAGE), 0);
        report(&heap, "and the advice again");
        mmap(dst, class, RW, ANON | MAP_NORESERVE | MAP_FIXED, -1, 0);
        report(&heap, "fresh mmap, same protection and flags, no advice");
        madvise(dst, class, MADV_NOHUGEPAGE);
        report(&heap, "then MADV_NOHUGEPAGE");

        let dst = moved(&heap);
        mmap(dst, class, RW, ANON | MAP_FIXED, -1, 0);
        madvise(dst, class, MADV_NOHUGEPAGE);
        report(&heap, "fresh mmap without MAP_NORESERVE, with advice");
        assert_eq!(heal(dst, class), 0);

        let dst = moved(&heap);
        mmap(dst, class, NONE, ANON | MAP_NORESERVE | MAP_FIXED, -1, 0);
        madvise(dst, class, MADV_NOHUGEPAGE);
        report(&heap, "fresh mmap PROT_NONE, with advice");
        mprotect(dst, class, RW);
        report(&heap, "then mprotect to read-write");

        // The source side. A move out of a slot the block was born in leaves its mapping whole.
        let dst = moved(&heap);
        println!(
            "  source of that move: {} mapping(s) across region 0's writable part",
            count_in(heap.base, COMMITTED)
        );
        // A move out of a slot the block was moved into takes all of that mapping's pages.
        let next = heap.slot(0, class, 100);
        assert_eq!(remap(dst, next, class), 0);
        report(&heap, "moved on from a moved-in slot, neither healed");
        println!("  the vacated slot reads zero: {}", is_zero(dst, class));
        assert_eq!(madvise(dst, class, MADV_DONTNEED), 0);
        report(&heap, "MADV_DONTNEED on the vacated slot");
        assert_eq!(heal(dst, class), 0);
        report(&heap, "fresh mmap and advice on the vacated slot");
        assert_eq!(heal(next, class), 0);
        report(&heap, "and on the last destination");

        // In-place growth after a move puts the block in two mappings; the kernel moves both.
        let src = heap.slot(0, class, 200);
        let dst = heap.slot(1, class, 300);
        let then = heap.slot(0, class, 210);
        fill(src, MB, 1);
        assert_eq!(remap(src, dst, MB), 0);
        fill(dst, class, 5);
        println!(
            "  a block that grew in place after arriving lies in {} mappings; probe says errno {}",
            count_in(dst, class),
            probe(dst, class)
        );
        let e = remap(dst, then, class);
        println!(
            "  moving all of it in one call: errno {e}, data intact {}, and it lands as {} mappings",
            holds(then, class, 5),
            count_in(then, class)
        );
        assert_eq!(heal(dst, class), 0);
        assert_eq!(heal(then, class), 0);
        report(&heap, "both healed over the whole block");
    }
    heap.drop();

    // A healed range merges with a neighbour only if they do not carry different
    // anonymous-memory bookkeeping (the kernel's anon_vma). Two ways to end up with a
    // neighbour on each side that differ, each leaving a split no heal removes.
    let heap = Heap::new(&[class, class, class, class]);
    let mut baseline = heap.count();
    let mut report = |heap: &Heap, what: &str| {
        println!("  {what}: {} extra mappings", heap.count() as isize - baseline as isize);
        baseline = heap.count();
    };
    // (a) Something is moved into a region before anything was written there, and the two
    // sides are first written afterwards.
    let src = heap.slot(0, class, 1);
    fill(src, class, 1);
    let dst = heap.slot(1, class, 5);
    assert_eq!(remap(src, dst, class - PAGE), 0);
    fill(heap.slot(1, class, 4), class, 2);
    fill(heap.slot(1, class, 6), class, 3);
    assert_eq!(heal(dst, class - PAGE), 0);
    report(&heap, "moved into a region never written, sides written later, healed");
    // The same when the region was written once first.
    touch(heap.slot(2, class, 0), PAGE, PAGE, 1);
    unsafe { madvise(heap.slot(2, class, 0), PAGE, MADV_DONTNEED) };
    let src = heap.slot(0, class, 2);
    fill(src, class, 1);
    let dst = heap.slot(2, class, 5);
    assert_eq!(remap(src, dst, class - PAGE), 0);
    fill(heap.slot(2, class, 4), class, 2);
    fill(heap.slot(2, class, 6), class, 3);
    assert_eq!(heal(dst, class - PAGE), 0);
    report(&heap, "the same in a region written once (and purged) beforehand");
    // (b) A healed range with a moved-in neighbour on both sides is written before they heal.
    touch(heap.slot(3, class, 0), PAGE, PAGE, 1);
    let dsts = [10, 11, 12].map(|i| heap.slot(3, class, i));
    for (k, &dst) in dsts.iter().enumerate() {
        let src = heap.slot(0, class, 20 + k);
        fill(src, class, 1);
        assert_eq!(remap(src, dst, class), 0);
    }
    assert_eq!(heal(dsts[1], class), 0);
    fill(dsts[1], class, 7);
    assert_eq!(heal(dsts[0], class), 0);
    assert_eq!(heal(dsts[2], class), 0);
    report(&heap, "three whole slots moved in, the middle healed and written first");
    heap.drop();
}

/// Whether the cycle test should leave out one part of the recipe (`REMAP_WITHOUT=identity`,
/// `lastpage` or `exact`), to show what each is for.
fn without(part: &str) -> bool {
    std::env::var("REMAP_WITHOUT").is_ok_and(|v| v.split(',').any(|p| p == part))
}

/// One slot of the cycle test, which models the design the study recommends: a slot holds at
/// most one moved-in range, a move makes at most one `mremap` call on a range the kernel says
/// is one mapping, the rest is copied skipping zero pages, and a slot's moved-in range is
/// replaced by a fresh mapping when the slot is given up.
#[derive(Clone, Copy, Default)]
struct Slot {
    /// Bytes in use, or zero if free.
    used: usize,
    seed: u64,
    /// The byte range of the slot that was moved in, if any.
    moved_in: Option<(usize, usize)>,
}

struct Cycles {
    heap: Heap,
    classes: Vec<usize>,
    slots: Vec<Vec<Slot>>,
    rng: Rng,
    baseline: usize,
    worst: (isize, usize),
    /// The most extra mappings per moved-in slot seen at any step.
    ratio: f64,
    moves: usize,
    remapped: usize,
    /// Moves whose largest piece the kernel said was not one mapping.
    split: usize,
    refused: usize,
}

impl Cycles {
    const SLOTS: usize = 48;
    /// The smallest piece the model moves by remapping.
    const LEAST: usize = 64 * KB;

    fn new() -> Cycles {
        let classes = vec![64 * KB, 448 * KB, MB, 3 * MB, 8 * MB, 40 * MB];
        let heap = Heap::new(&classes);
        // One written page gives each region's mapping its anonymous-memory identity before
        // anything is moved into it; see the write-up for why that matters.
        if !without("identity") {
            for (r, &class) in classes.iter().enumerate() {
                touch(heap.slot(r, class, 0), PAGE, PAGE, 1);
                unsafe { madvise(heap.slot(r, class, 0), PAGE, MADV_DONTNEED) };
            }
        }
        let baseline = heap.count();
        let slots = vec![vec![Slot::default(); Self::SLOTS]; classes.len()];
        Cycles {
            heap,
            classes,
            slots,
            rng: Rng(0x9e3779b97f4a7c15),
            baseline,
            worst: (0, 0),
            ratio: 0.0,
            moves: 0,
            remapped: 0,
            split: 0,
            refused: 0,
        }
    }

    fn at(&self, r: usize, i: usize) -> *mut u8 {
        self.heap.slot(r, self.classes[r], i)
    }

    /// Sparse contents: a word on every eighth page.
    fn write(&self, r: usize, i: usize, from: usize, to: usize, seed: u64) {
        let p = self.at(r, i);
        for page in (from / PAGE..to / PAGE).step_by(8) {
            unsafe { (p.add(page * PAGE) as *mut u64).write_volatile(seed ^ page as u64 | 1) };
        }
    }

    fn verify(&self, r: usize, i: usize) {
        let s = self.slots[r][i];
        let p = self.at(r, i);
        for page in 0..s.used / PAGE {
            let got = unsafe { (p.add(page * PAGE) as *const u64).read_volatile() };
            let want = if page % 8 == 0 { s.seed ^ page as u64 | 1 } else { 0 };
            assert_eq!(got, want, "region {r} slot {i} page {page}");
        }
    }

    fn free_slot(&mut self, r: usize, i: usize) {
        let class = self.classes[r];
        let p = self.at(r, i);
        assert_eq!(unsafe { madvise(p, class, MADV_DONTNEED) }, 0);
        if let Some((a, b)) = self.slots[r][i].moved_in {
            let (a, b) = if without("exact") { (0, class) } else { (a, b) };
            assert_eq!(heal(unsafe { p.add(a) }, b - a), 0);
        }
        self.slots[r][i] = Slot::default();
    }

    fn pick(&mut self, r: usize, want_used: bool) -> Option<usize> {
        let start = self.rng.below(Self::SLOTS);
        (0..Self::SLOTS).map(|k| (start + k) % Self::SLOTS).find(|&i| (self.slots[r][i].used != 0) == want_used)
    }

    fn move_block(&mut self, r: usize, i: usize, to: usize, j: usize) {
        let s = self.slots[r][i];
        let (from, dest) = (self.at(r, i), self.at(to, j));
        let len = s.used.min(self.classes[to]);
        // The pieces of the block that can each be one mapping: before, inside and after the
        // source's moved-in range. The last page of the destination slot is never remapped.
        let limit = if without("lastpage") { len } else { len.min(self.classes[to] - PAGE) };
        let (a, b) = s.moved_in.unwrap_or((0, 0));
        let pieces = [(0, a.min(limit)), (a.min(limit), b.min(limit)), (b.min(limit), limit)];
        let (start, end) = pieces.into_iter().max_by_key(|&(s, e)| e - s).unwrap();
        let mut moved = None;
        if end - start >= Self::LEAST {
            let range = unsafe { from.add(start) };
            if probe(range, end - start) != ENOMEM {
                self.split += 1;
            } else if remap(range, unsafe { dest.add(start) }, end - start) == 0 {
                moved = Some((start, end));
                self.remapped += 1;
            } else {
                self.refused += 1;
            }
        }
        let (start, end) = moved.unwrap_or((0, 0));
        unsafe {
            copy_sparse(from, dest, start);
            copy_sparse(from.add(end), dest.add(end), len - end);
        }
        self.moves += 1;
        self.slots[to][j] = Slot { used: len, seed: s.seed, moved_in: moved };
        self.verify(to, j);
        self.free_slot(r, i);
    }

    fn step(&mut self) {
        let r = self.rng.below(self.classes.len());
        let class = self.classes[r];
        match self.rng.below(8) {
            0 | 1 => {
                if let Some(i) = self.pick(r, false) {
                    let used = (class / 4 + self.rng.below(3 * class / 4 + 1)) & !(8 * PAGE - 1);
                    let seed = self.rng.next();
                    self.slots[r][i] = Slot { used, seed, moved_in: None };
                    self.write(r, i, 0, used, seed);
                }
            }
            2 => {
                if let Some(i) = self.pick(r, true) {
                    self.verify(r, i);
                    self.free_slot(r, i);
                }
            }
            3 => {
                // Grow in place, which faults pages of the region's own mapping after the
                // part that was moved in.
                if let Some(i) = self.pick(r, true) {
                    let s = self.slots[r][i];
                    let used = (s.used + self.rng.below(class - s.used + 1)) & !(8 * PAGE - 1);
                    if used > s.used {
                        self.write(r, i, s.used, used, s.seed);
                        self.slots[r][i].used = used;
                    }
                }
            }
            _ => {
                // Move to a slot of another region, shrinking if it does not fit.
                let Some(i) = self.pick(r, true) else { return };
                let to = self.rng.below(self.classes.len());
                let Some(j) = self.pick(to, false) else { return };
                self.move_block(r, i, to, j);
            }
        }
        let extra = self.heap.count() as isize - self.baseline as isize;
        let moved_in = self.slots.iter().flatten().filter(|s| s.moved_in.is_some()).count();
        if extra > self.worst.0 {
            self.worst = (extra, moved_in);
        }
        if moved_in > 0 {
            self.ratio = self.ratio.max(extra as f64 / moved_in as f64);
        }
    }

    fn free_all(&mut self) {
        for r in 0..self.classes.len() {
            for i in 0..Self::SLOTS {
                if self.slots[r][i].used != 0 {
                    self.verify(r, i);
                    self.free_slot(r, i);
                }
            }
        }
    }

    fn run(&mut self, rounds: usize, steps: usize, label: &str) {
        for round in 0..rounds {
            for _ in 0..steps {
                self.step();
            }
            let live = self.slots.iter().flatten().filter(|s| s.used != 0).count();
            let moved_in = self.slots.iter().flatten().filter(|s| s.moved_in.is_some()).count();
            let busy = self.heap.count();
            self.free_all();
            println!(
                "  {label} round {round}: {} moves so far, {} with a remap ({} refused, {} not one mapping); {live} live, {moved_in} moved-in, {busy} mappings; after freeing all: {} (baseline {})",
                self.moves,
                self.remapped,
                self.refused,
                self.split,
                self.heap.count(),
                self.baseline
            );
        }
        println!(
            "  {label}: most extra mappings {} with {} moved-in slots; most per moved-in slot {:.2}",
            self.worst.0, self.worst.1, self.ratio
        );
    }
}

fn cycles() {
    println!("== cycles: random allocate, grow, move and free across six classes");
    let mut c = Cycles::new();
    c.run(5, 20_000, "one process");
    c.heap.show("at the end");
    if std::env::var("REMAP_WITHOUT").is_ok() {
        return;
    }

    // The same in a process that forked while blocks were live, on both sides of the fork.
    for _ in 0..3000 {
        c.step();
    }
    let before = c.heap.count();
    println!("  forking with {before} mappings");
    let code = in_child(|| {
        c.free_all();
        println!(
            "  child, everything freed and healed: {} mappings (before the fork: {before}, baseline {})",
            c.heap.count(),
            c.baseline
        );
        c.worst = (0, 0);
        c.ratio = 0.0;
        c.run(5, 20_000, "child");
        c.heap.show("child at the end");
        0
    });
    println!("  child exit {code}");
    c.free_all();
    println!("  parent after the fork, everything freed: {} mappings (baseline {})", c.heap.count(), c.baseline);
    c.run(2, 20_000, "parent after fork");
}

fn fork_tests() {
    println!("== fork: a move with a fork around it");
    let class = 4 * MB;
    let heap = Heap::new(&[class, class]);
    let (a, b, c) = (heap.slot(0, class, 3), heap.slot(1, class, 3), heap.slot(1, class, 9));
    fill(a, class, 11);
    fill(heap.slot(1, class, 2), class, 12);
    let baseline = heap.count();
    // Fork, then the parent moves: the child must keep seeing the block where it was.
    let mut gate = [0i32; 2];
    let mut done = [0i32; 2];
    extern "C" {
        fn pipe(fds: *mut i32) -> i32;
        fn read(fd: i32, buf: *mut std::ffi::c_void, n: usize) -> isize;
        fn write(fd: i32, buf: *const std::ffi::c_void, n: usize) -> isize;
    }
    unsafe {
        pipe(gate.as_mut_ptr());
        pipe(done.as_mut_ptr());
        let pid = fork();
        if pid == 0 {
            let mut byte = 0u8;
            read(gate[0], (&raw mut byte).cast(), 1);
            // The parent has moved a -> b by now.
            let ok = holds(a, class, 11) && is_zero(b, class);
            // The child moves its own copy elsewhere and heals, and counts.
            let e = remap(a, c, class);
            let moved = holds(c, class, 11) && is_zero(a, class);
            let extra = heap.count() as isize - baseline as isize;
            heal(c, class);
            let healed = heap.count() as isize - baseline as isize;
            println!("  child: still sees the block at its old address {ok}; its own move errno {e} intact {moved}; +{extra} mappings, +{healed} after heal");
            _exit(0);
        }
        assert_eq!(remap(a, b, class), 0);
        write(gate[1], [1u8].as_ptr().cast(), 1);
        let mut status = 0;
        waitpid(pid, &mut status, 0);
        println!(
            "  parent: block intact at the destination {}; source zero {}",
            holds(b, class, 11),
            is_zero(a, class)
        );
        assert_eq!(heal(b, class), 0);
        println!("  parent after heal: {} mappings (baseline {baseline})", heap.count());
    }

    // The same question for growing a region in a forked child, which the allocator already
    // does: does each mprotect step leave a mapping behind?
    let region = unsafe { heap.base.add(3 * REGION) };
    prepare_region(region, REGION, 64 * KB);
    touch(region, 64 * KB, PAGE, 1);
    let before = count_in(region, REGION);
    in_child(|| {
        let mut committed = 64 * KB;
        for step in 0..64 {
            unsafe { mprotect(region.add(committed), 64 * KB, RW) };
            if step % 2 == 0 {
                touch(unsafe { region.add(committed) }, 64 * KB, PAGE, 1);
            }
            committed += 64 * KB;
        }
        println!(
            "  child growing an inherited region by 64 mprotect steps: {} -> {} mappings in the region",
            before,
            count_in(region, REGION)
        );
        0
    });
    heap.drop();
}

fn probe_test() {
    println!("== probe: asking whether a range lies in one mapping");
    let class = 4 * MB;
    let heap = Heap::new(&[class, class]);
    let a = heap.slot(0, class, 3);
    fill(a, class, 3);
    let before = heap.count();
    println!("  a slot in the region's own mapping: errno {} (ENOMEM = one mapping)", probe(a, class));
    let b = heap.slot(1, class, 5);
    assert_eq!(remap(a, b, MB), 0);
    println!("  a moved-in range exactly: errno {}", probe(b, MB));
    println!("  part of it: errno {}", probe(b, MB / 2));
    println!("  the moved-in range and what follows it: errno {} (EFAULT = more than one)", probe(b, class));
    println!("  the tail after it: errno {}", probe(unsafe { b.add(MB) }, class - MB));
    println!("  mappings unchanged by probing: {}; contents intact: {}", heap.count() == before + 2, holds(b, MB, 3));
    // What the application can do to a block it holds.
    let c = heap.slot(0, class, 20);
    fill(c, class, 4);
    unsafe { madvise(c.add(MB), MB, MADV_HUGEPAGE) };
    println!("  a block the program advised in part: errno {}", probe(c, class));
    // The last slot before unmapped space: the probe must not be used there.
    let lone = reserve(64 * MB, HUGE);
    unsafe { mprotect(lone, 64 * MB, RW) };
    fill(lone, 4 * MB, 1);
    unsafe { munmap(lone.add(32 * MB), 32 * MB) };
    let e = probe(unsafe { lone.add(28 * MB) }, 4 * MB);
    println!(
        "  a range ending where unmapped space begins: errno {e}, mapping now {} MiB long",
        maps_in(lone, 64 * MB)[0].end.wrapping_sub(lone as usize) / MB
    );
    heap.drop();
}

/// Whether a page is mapped, asked without allocating: `madvise` fails with ENOMEM on a hole.
fn mapped(at: *mut u8) -> bool {
    unsafe { madvise(at, PAGE, 0) == 0 }
}

/// Mappings that each add exactly one to the process's count and can be taken away again in
/// reverse order: the leading pages of an inaccessible reservation, alternately readable and
/// writable.
struct Singles {
    base: *mut u8,
    made: usize,
}

impl Singles {
    fn add(&mut self) -> bool {
        let prot = if self.made % 2 == 0 { 1 } else { RW };
        let ok = unsafe { mprotect(self.base.add(self.made * PAGE), PAGE, prot) } == 0;
        self.made += ok as usize;
        ok
    }

    fn remove(&mut self) {
        self.made -= 1;
        assert_eq!(unsafe { mprotect(self.base.add(self.made * PAGE), PAGE, NONE) }, 0);
    }
}

fn mapcount() {
    println!("== mapcount: moves near vm.max_map_count");
    let max: usize = std::fs::read_to_string("/proc/sys/vm/max_map_count").unwrap().trim().parse().unwrap();
    let class = 4 * MB;
    let heap = Heap::new(&[class, class]);
    for i in 0..16 {
        touch(heap.slot(1, class, i), class, PAGE, 0xEE);
    }
    // A block in one mapping, and a block in three (its middle page read-only).
    let one = heap.slot(0, class, 9);
    let three = heap.slot(0, class, 3);
    fill(one, class, 31);
    fill(three, 3 * PAGE, 21);
    unsafe { mprotect(three.add(PAGE), PAGE, 1) };
    let mut singles = Singles { base: reserve(64 * PAGE, PAGE), made: 0 };
    for _ in 0..24 {
        assert!(singles.add());
    }
    // Fill the rest two at a time, then one at a time, until the kernel refuses a split: the
    // count is then exactly at the limit.
    let filler = reserve(2 * max * PAGE, PAGE);
    let mut i = 0;
    while unsafe { mprotect(filler.add((2 * i + 1) * PAGE), PAGE, 1) } == 0 {
        i += 1;
    }
    while singles.add() {}
    // Reading the list needs memory, so step back from the limit while counting.
    for _ in 0..16 {
        singles.remove();
    }
    let lines = maps().len() + 16;
    while singles.add() {}
    println!("  at the limit of {max}: {lines} lines in /proc/self/maps");
    let state = |page: *mut u8, dst: *mut u8, seed: u64| -> i32 {
        if !mapped(dst) {
            return 2;
        }
        let d = unsafe { (dst as *const u64).read_volatile() };
        let s = unsafe { (page as *const u64).read_volatile() };
        match (d == seed, s == seed) {
            (true, false) => 0,
            (false, true) => 1,
            _ => 3,
        }
    };
    let names = ["moved", "not moved", "not moved and the destination unmapped", "lost"];
    for spare in 2..=8usize {
        let code = in_child(|| {
            for _ in 0..spare {
                singles.remove();
            }
            let dst = heap.slot(1, class, 3);
            let e = remap(one, dst, class);
            let s = state(one, dst, 31);
            // Room to report in.
            for _ in 0..16 {
                singles.remove();
            }
            println!("  {spare} below the limit, a block in one mapping: errno {e}; {}", names[s as usize]);
            0
        });
        assert_eq!(code, 0);
        let code = in_child(|| {
            for _ in 0..spare {
                singles.remove();
            }
            let dst = heap.slot(1, class, 3);
            let e = remap(three, dst, 3 * PAGE);
            let pages =
                [0, 1, 2].map(|k| state(unsafe { three.add(k * PAGE) }, unsafe { dst.add(k * PAGE) }, 21 + k as u64));
            for _ in 0..16 {
                singles.remove();
            }
            println!(
                "  {spare} below the limit, a block in three mappings: errno {e}; pages: {}; {}; {}",
                names[pages[0] as usize], names[pages[1] as usize], names[pages[2] as usize]
            );
            0
        });
        assert_eq!(code, 0);
    }
    // Healing at the limit: move with room, use the room up, then heal.
    let code = in_child(|| {
        for _ in 0..8 {
            singles.remove();
        }
        let dst = heap.slot(1, class, 3);
        assert_eq!(remap(one, dst, MB), 0);
        while singles.add() {}
        let exact = heal(dst, MB);
        while singles.add() {}
        let dst2 = heap.slot(1, class, 7);
        let whole = unsafe { mmap(dst2, class, RW, ANON | MAP_NORESERVE | MAP_FIXED, -1, 0) };
        let whole = if whole == dst2 { 0 } else { errno() };
        let still = mapped(dst2);
        for _ in 0..16 {
            singles.remove();
        }
        println!("  at the limit: a fresh mapping over exactly the moved-in range: errno {exact}; over a slot inside a larger mapping: errno {whole}, still mapped {still}");
        0
    });
    assert_eq!(code, 0);
}

fn hole() {
    println!("== hole: a move the kernel refuses after it has unmapped the destination");
    // An accounted mapping larger than RAM and swap, built from two halves that each pass the
    // heuristic overcommit check. Moving it must charge its whole size again, which fails.
    let total_kb = {
        let text = std::fs::read_to_string("/proc/meminfo").unwrap();
        let get = |k: &str| {
            text.lines()
                .find(|l| l.starts_with(k))
                .and_then(|l| l.split_whitespace().nth(1))
                .and_then(|v| v.parse::<usize>().ok())
                .unwrap()
        };
        get("MemTotal:") + get("SwapTotal:")
    };
    let half = ((total_kb * KB) * 3 / 4).next_multiple_of(GB);
    let span = 5 * half;
    let base = unsafe { mmap(std::ptr::null_mut(), span, NONE, ANON, -1, 0) };
    assert!(base as isize != -1);
    unsafe {
        assert_eq!(mprotect(base, half, RW), 0);
        assert_eq!(mprotect(base.add(half), half, RW), 0);
        fill(base, MB, 5);
        let dst = base.add(3 * half);
        show("before", base, &maps_in(base, span));
        let e = remap(base, dst, 2 * half);
        println!("  mremap of {} GiB accounted: errno {e}", 2 * half / GB);
        show("after", base, &maps_in(base, span));
        println!("  source intact: {}; destination mapped: {}", holds(base, MB, 5), !maps_in(dst, 2 * half).is_empty());
        let p = mmap(dst, 2 * half, NONE, ANON | MAP_FIXED, -1, 0);
        println!(
            "  mapping the destination again: {}",
            if p == dst { "ok".to_string() } else { format!("errno {}", errno()) }
        );
        munmap(base, span);
    }
}

fn mlock_test() {
    println!("== mlock: what a move does to a locked source mapping");
    let class = MB;
    let heap = Heap::new(&[class, class]);
    let region = heap.slot(0, class, 0);
    // Lock four slots' worth, as mlockall would have locked the whole heap.
    fill(region, 4 * class, 1);
    let e = check(unsafe { mlock(region, 4 * class) });
    println!("  mlock of 4 MiB: errno {e}; flags {}", smaps_at(region).0);
    let src = heap.slot(0, class, 1);
    let dst = heap.slot(1, class, 1);
    println!(
        "  MADV_COLD on a locked page: errno {} (EINVAL = {EINVAL}); MADV_DONTNEED: errno {}",
        check(unsafe { madvise(src, PAGE, MADV_COLD) }),
        check(unsafe { madvise(heap.slot(0, class, 3), PAGE, MADV_DONTNEED) })
    );
    println!("  MADV_COLD on an unlocked page: errno {}", check(unsafe { madvise(dst, PAGE, MADV_COLD) }));
    let e = remap(src, dst, class);
    println!("  move of one slot out of the locked mapping: errno {e}");
    println!("  flags of the source mapping (all four slots): {}", smaps_at(region).0);
    println!("  flags of the destination: {}", smaps_at(dst).0);
    heap.drop();
}

fn thp() {
    println!("== thp: huge pages in the source or the destination");
    let heap = Heap::new(&[8 * MB, 8 * MB, 3 * MB]);
    unsafe {
        // Source with huge pages, both ends aligned to 2 MiB.
        let src = heap.slot(0, 8 * MB, 2);
        madvise(src, 8 * MB, MADV_HUGEPAGE);
        fill(src, 8 * MB, 1);
        let (_, rss, huge) = smaps_at(src);
        let dst = heap.slot(1, 8 * MB, 2);
        let e = remap(src, dst, 8 * MB);
        let (flags, rss2, huge2) = smaps_at(dst);
        println!("  aligned: source {rss} kB resident, {huge} kB huge; move errno {e}; destination {rss2} kB resident, {huge2} kB huge; intact {}; flags {flags}", holds(dst, 8 * MB, 1));
        // The same to a destination that is not aligned.
        let src = heap.slot(0, 8 * MB, 5);
        madvise(src, 8 * MB, MADV_HUGEPAGE);
        fill(src, 8 * MB, 2);
        let huge = smaps_at(src).2;
        let dst = heap.slot(2, 3 * MB, 3);
        let e = remap(src, dst, 6 * MB);
        let (_, rss2, huge2) = smaps_at(dst);
        println!("  unaligned destination: source {huge} kB huge; move of 6 MiB errno {e}; destination {rss2} kB resident, {huge2} kB huge; intact {}", holds(dst, 6 * MB, 2));
        // A destination that holds huge pages is simply replaced.
        let src = heap.slot(0, 8 * MB, 9);
        fill(src, 8 * MB, 3);
        let dst = heap.slot(1, 8 * MB, 9);
        madvise(dst, 8 * MB, MADV_HUGEPAGE);
        touch(dst, 8 * MB, PAGE, 0xEE);
        let huge = smaps_at(dst).2;
        let e = remap(src, dst, 6 * MB);
        println!("  destination with {huge} kB huge: move of 6 MiB errno {e}; intact {}; the 2 MiB past it still the old bytes {}", holds(dst, 6 * MB, 3), *dst.add(6 * MB) == 0xEE);
    }
    heap.drop();
}

fn cgroup() {
    println!("== cgroup: a move when the memory cap leaves no room for a second copy");
    let peak = || {
        let path = std::fs::read_to_string("/proc/self/cgroup").unwrap();
        let path = path.trim().trim_start_matches("0::");
        std::fs::read_to_string(format!("/sys/fs/cgroup{path}/memory.peak"))
            .ok()
            .and_then(|v| v.trim().parse::<u64>().ok())
            .unwrap_or(0)
            / MB as u64
    };
    let size: usize = std::env::var("REMAP_MB").ok().and_then(|v| v.parse().ok()).unwrap_or(400) * MB;
    let heap = Heap::new(&[GB, GB, GB]);
    let src = heap.slot(0, GB, 0);
    fill(src, size, 1);
    println!("  {} MiB written; cgroup peak {} MiB", size / MB, peak());
    let dst = heap.slot(1, GB, 1);
    let e = remap(src, dst, size);
    println!("  aligned move: errno {e}; intact {}; cgroup peak {} MiB", holds(dst, size, 1), peak());
    // Unaligned: the kernel must allocate page tables for the destination.
    let odd = unsafe { heap.slot(2, GB, 0).add(PAGE) };
    let e = remap(dst, odd, size);
    println!("  unaligned move: errno {e}; intact {}; cgroup peak {} MiB", holds(odd, size, 1), peak());
    heap.drop();
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let all = ["basic", "ingredients", "cycles", "fork", "probe", "hole", "mlock", "thp"];
    let chosen: Vec<&str> = if args.is_empty() { all.to_vec() } else { args.iter().map(String::as_str).collect() };
    for test in chosen {
        match test {
            "basic" => basic(),
            "ingredients" => ingredients(),
            "cycles" => cycles(),
            "fork" => fork_tests(),
            "probe" => probe_test(),
            "mapcount" => mapcount(),
            "hole" => hole(),
            "mlock" => mlock_test(),
            "thp" => thp(),
            "cgroup" => cgroup(),
            other => panic!("unknown test {other}"),
        }
    }
}
