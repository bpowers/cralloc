//! What the allocator's placement does to a program's walks: blocks at slot starts, classes on
//! the same page numbers, magazines popped in a sawtooth, and the array of pointers a magazine is.
//!
//! ```text
//! streams stagger CPU small|huge [streams] [KiB]  buffers written in turn, by how far each is moved
//! streams rule CPU small|huge                     the stagger the allocator would use, by class
//! streams origin CPU                              classes' first pages in a region of 4 GiB each
//! streams sawtooth CPU [MiB]                      a walk in the order magazines hand objects out
//! streams pointers CPU [MiB]                      an array of pointers against one of offsets
//! streams magazine CPU [MiB]                       the same in pops of magazines of each size
//! ```
use roofline_machine::*;

fn parse_pages(s: Option<&String>) -> Pages {
    match s.map(String::as_str) {
        Some("huge") => Pages::Huge,
        _ => Pages::Small,
    }
}

/// `count` buffers of `each` bytes in slots of `slot` bytes, each moved by `step` times its index
/// modulo `slots`, written one word at a time in turn, as vectors filled together are.
fn stagger_pass(map: &Map, count: usize, each: usize, slot: usize, step: usize, slots: usize) -> f64 {
    let starts: Vec<*mut u64> =
        (0..count).map(|i| map.p.wrapping_add(i * slot + step * (i % slots.max(1))).cast()).collect();
    let words = each / 8;
    let mut best = f64::MAX;
    for _ in 0..3 {
        let t0 = ticks();
        for w in 0..words {
            for &s in &starts {
                // SAFETY: each buffer holds `words` words inside the mapping, past its stagger.
                unsafe { s.add(w).write_volatile(w as u64) };
            }
        }
        best = best.min(ns_of(ticks() - t0) / (words * count) as f64);
    }
    best
}

/// Buffers of `each` bytes in slots of `slot` bytes, by stagger. A stagger that would not fit its
/// slot is left out.
fn stagger_table(map: &Map, pages: Pages, count: usize, each: usize, slot: usize) {
    let page = page();
    println!(
        "# {count} buffers of {} KiB in slots of {} KiB ({} pages), {pages:?} pages",
        each >> 10,
        slot >> 10,
        slot / page
    );
    println!("# step  slots  span  ns_an_element");
    let cells = [
        (0usize, 1usize),
        (8, 64),
        (32, 64),
        (64, 8),
        (64, 16),
        (64, 64),
        (64, 256),
        (128, 16),
        (128, 64),
        (128, 128),
        (256, 64),
        (4096, 16),
        (page, 8),
        // A page and a line: each buffer on its own TLB set as well as its own cache set.
        (page + 64, 4),
        (page + 64, 8),
        (page + 64, 16),
        (page + 64, 32),
        (page + 64, 64),
        (page + 64, 128),
        (2 * page + 64, 64),
        (5 * page + 64, 64),
    ];
    let cells: Vec<(usize, usize)> = cells.into_iter().filter(|&(step, slots)| step * slots + each <= slot).collect();
    let mut order: Vec<usize> = (0..cells.len()).collect();
    let mut results = vec![f64::MAX; cells.len()];
    let mut rng = Rng(0x1357_2468);
    for _ in 0..3 {
        rng.shuffle(&mut order);
        for &i in &order {
            let (step, slots) = cells[i];
            results[i] = results[i].min(stagger_pass(map, count, each, slot, step, slots));
        }
    }
    for (i, &(step, slots)) in cells.iter().enumerate() {
        println!("{step:6} {slots:5} {:7} {:8.2}", step * slots, results[i]);
    }
}

fn stagger(cpu: usize, pages: Pages, count: usize, kib: usize) {
    pin(cpu);
    let each = kib << 10;
    // A slot as a large class has it: the next class up from the block and a little room, of four
    // a doubling, so a power of two only when the block is near one. Then a power of two.
    let room = 64 * 64;
    let want = each + room;
    let round = want.next_power_of_two();
    let quarter = round / 8;
    let class = (want.div_ceil(quarter)) * quarter;
    for slot in [class, round, 2 * round] {
        let map = Map::resident(count * slot + slot, pages);
        println!("# huge pages back {} MiB", huge_mib());
        stagger_table(&map, pages, count, each, slot);
    }
}

/// Pages of the page term for a slot of `pages` pages: an eighth of the slot, up to 32, and none
/// when that is under two. So the room is at most an eighth of the slot, and a class moves up by
/// at most one step for it.
fn page_term(pages: usize) -> usize {
    let p = (pages / 8).min(32);
    if p < 2 { 0 } else { p }
}

/// Elements written in turn to `starts`, `words` from each: nanoseconds an element, the best of
/// three passes.
fn written_in_turn(starts: &[*mut u64], words: usize) -> f64 {
    let mut best = f64::MAX;
    for _ in 0..3 {
        let t0 = ticks();
        for w in 0..words {
            for &s in starts {
                // SAFETY: each start has `words` words inside the mapping, by the caller.
                unsafe { s.add(w).write_volatile(w as u64) };
            }
        }
        best = best.min(ns_of(ticks() - t0) / (words * starts.len()) as f64);
    }
    best
}

/// The rule as the allocator would have it, for every class of four a doubling from 8 to 1024 pages:
/// no stagger, the cache lines' alone, and both terms.
fn rule(cpu: usize, pages_kind: Pages) {
    pin(cpu);
    let page = page();
    println!(
        "# stagger(i) = 64 * (i % 64) + page * (i % P), P = page_term(slot); CPU {cpu}, page {page}, {pages_kind:?} pages"
    );
    println!("# slot_pages slot_KiB  P  room_KiB  ns_an_element by streams, each: none, lines, rule");
    let classes = [
        8usize, 10, 12, 14, 16, 20, 24, 28, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 384, 448,
        512, 640, 768, 1024,
    ];
    for &c in &classes {
        let slot = c * page;
        if (slot * 64) >> 20 > max_mib(6144) {
            break;
        }
        let map = Map::resident(64 * slot + slot, pages_kind);
        let p = page_term(c);
        let room = 4096 + p * page;
        // Only the part of each block every class can write in the same time.
        let words = (slot.saturating_sub(room) / 8).min((1 << 20) / 8);
        print!("{c:6} {:8} {p:3} {:6}", slot >> 10, room >> 10);
        for streams in [16usize, 64] {
            let at = |offset: &dyn Fn(usize) -> usize| -> Vec<*mut u64> {
                (0..streams).map(|i| map.p.wrapping_add(i * slot + offset(i)).cast()).collect()
            };
            let none = written_in_turn(&at(&|_| 0), words);
            let lines = written_in_turn(&at(&|i| 64 * (i % 64)), words);
            let both = written_in_turn(&at(&|i| 64 * (i % 64) + if p > 0 { page * (i % p) } else { 0 }), words);
            print!("   {none:6.2} {lines:6.2} {both:6.2}");
        }
        println!();
    }
}

/// `classes` regions 4 GiB apart, each using its first `pages_each` pages from its origin, one
/// line a page, visited in random order: when the origins do not spread them, they fill a few sets
/// of the TLBs.
fn origin(cpu: usize) {
    pin(cpu);
    let cycles = Counter::cycles(cpu);
    let page = page();
    const REGION: usize = 1 << 32;
    let classes_most = 64;
    let map = Map::new(classes_most * REGION + (1 << 34), Pages::Small);
    // Regions aligned to their size, as a heap's are.
    let base = map.p.map_addr(|a| (a + REGION - 1) & !(REGION - 1));
    println!(
        "# classes at a region's size apart, CPU {cpu}, page {page}: cycles a load, by span of the origins in pages"
    );
    let spans = [0usize, 16, 32, 64, 128, 256, 512, 1024];
    print!("# classes pages_each");
    for s in spans {
        print!(" {s:6}");
    }
    println!();
    let step_for = |span: usize| -> usize {
        // The odd number nearest the span over the golden ratio.
        ((span as f64 / std::f64::consts::GOLDEN_RATIO) as usize) | 1
    };
    for &(classes, pages_each) in
        &[(8usize, 1usize), (16, 1), (32, 1), (64, 1), (16, 4), (32, 4), (64, 4), (16, 16), (32, 16), (64, 16)]
    {
        print!("{classes:9} {pages_each:10}");
        for &span in &spans {
            let mut colours = Rng(0xabab_cdcd + span as u64);
            let mut at = Vec::with_capacity(classes * pages_each);
            for c in 0..classes {
                let first = if span == 0 { 0 } else { c * step_for(span) % span };
                for k in 0..pages_each {
                    let colour = (colours.next() as usize % (page / 128)) * 128;
                    at.push(base.wrapping_add(c * REGION + (first + k) * page + colour));
                }
            }
            Rng(0x2222_3333).shuffle(&mut at);
            let t = time_chase(link(&at), 1 << 18, at.len() * 8, 3, &cycles);
            print!(" {:6.1}", t.cycles);
        }
        println!();
    }
}

/// Objects in the order magazines of `run` objects hand them out, for each order of interest.
fn sawtooth(cpu: usize, mib: usize) {
    pin(cpu);
    let cycles = Counter::cycles(cpu);
    let len = mib << 20;
    let map = Map::resident(len, Pages::Huge);
    println!("# a chase through objects, {mib} MiB, huge pages back {} MiB, CPU {cpu}", huge_mib());
    println!("# size  order  run  cycles_an_object  ns_an_object  bytes_a_load");
    for size in [24usize, 64, 128, 256] {
        let count = (len / size).min(1 << 24);
        let at_of = |i: usize| map.p.wrapping_add(i * size);
        let report = |name: &str, run: usize, order: Vec<usize>| {
            let at: Vec<*mut u8> = order.iter().map(|&i| at_of(i)).collect();
            let start = link(&at);
            let loads = at.len();
            let t = time_chase(start, loads, loads, 2, &cycles);
            println!("{size:5} {name:10} {run:5} {:8.2} {:8.2}", t.cycles, t.ns);
        };
        report("ascending", 0, (0..count).collect());
        report("descending", 0, (0..count).rev().collect());
        for run in [14usize, 30, 62, 126, 254, 510, 1022] {
            // Successive magazines ascend, and each hands its objects out highest first.
            let mut order = Vec::with_capacity(count);
            let mut m = 0;
            while (m + 1) * run <= count {
                order.extend((0..run).rev().map(|i| m * run + i));
                m += 1;
            }
            report("sawtooth", run, order);
        }
        let mut random: Vec<usize> = (0..count).collect();
        Rng(0x7654_3210).shuffle(&mut random);
        report("random", 0, random);
    }
}

/// A data-memory-dependent prefetcher follows values in an array that look like pointers. The two
/// loops here read the same objects through the same order; one array holds their addresses, as a
/// magazine does, and the other holds offsets that look like nothing. Each load of an object feeds
/// the next index, so a target that comes early shows.
fn pointers(cpu: usize, mib: usize) {
    pin(cpu);
    let cycles = Counter::cycles(cpu);
    let len = mib << 20;
    let map = Map::resident(len, Pages::Huge);
    let line = line(cpu);
    let count = (len / line).min(1 << 21);
    let mut order: Vec<usize> = (0..len / line).collect();
    Rng(0xfeed_face_cafe_1234).shuffle(&mut order);
    order.truncate(count);
    let ptrs = Map::resident(count * 8, Pages::Huge);
    let offs = Map::resident(count * 4, Pages::Huge);
    let ptr_at = ptrs.p.cast::<usize>();
    let off_at = offs.p.cast::<u32>();
    for (i, &o) in order.iter().enumerate() {
        // SAFETY: both arrays hold `count` elements.
        unsafe {
            ptr_at.add(i).write(map.p.wrapping_add(o * line) as usize);
            off_at.add(i).write(o as u32);
        }
    }
    println!(
        "# {count} random objects of {mib} MiB through one array in order, huge pages back {} MiB, CPU {cpu}",
        huge_mib()
    );
    println!("# form  cycles_an_object  ns_an_object");
    let base = map.p as usize;
    let via_pointers = |up: bool| -> Timed {
        let (t0, c0) = (ticks(), cycles.read());
        let mut i = if up { 0 } else { count - 1 };
        for _ in 0..count {
            // SAFETY: `i` stays in the array; the buffer holds zero, so the sum's addend is zero.
            let v = unsafe { (ptr_at.add(i).read_volatile() as *const usize).read_volatile() };
            i = if up { i + 1 + v } else { i.wrapping_sub(1 + v) };
        }
        std::hint::black_box(i);
        let (c1, t1) = (cycles.read(), ticks());
        Timed { cycles: (c1 - c0) as f64 / count as f64, ns: ns_of(t1 - t0) / count as f64 }
    };
    let via_offsets = |up: bool| -> Timed {
        let (t0, c0) = (ticks(), cycles.read());
        let mut i = if up { 0 } else { count - 1 };
        for _ in 0..count {
            // SAFETY: as above.
            let v = unsafe {
                let o = off_at.add(i).read_volatile() as usize;
                ((base + o * line) as *const usize).read_volatile()
            };
            i = if up { i + 1 + v } else { i.wrapping_sub(1 + v) };
        }
        std::hint::black_box(i);
        let (c1, t1) = (cycles.read(), ticks());
        Timed { cycles: (c1 - c0) as f64 / count as f64, ns: ns_of(t1 - t0) / count as f64 }
    };
    for _ in 0..2 {
        // The buffer holds zero everywhere the walk reads.
        for (name, t) in [
            ("pointers up", via_pointers(true)),
            ("offsets up", via_offsets(true)),
            ("pointers down", via_pointers(false)),
            ("offsets down", via_offsets(false)),
        ] {
            println!("{name:14} {:8.2} {:8.2}", t.cycles, t.ns);
        }
    }
}

/// One pass over magazines of `len` entries, highest entry first, touching each object, with `WORK`
/// instructions between pops and no branch among them, which would hide the overlap behind its own
/// prediction. With `CHAIN` each object's value feeds the next pop, so that nothing overlaps.
#[inline(never)]
fn pop_pointers<const WORK: usize, const CHAIN: bool>(at: *const usize, mags: usize, len: usize) -> usize {
    let mut sum = 0usize;
    for m in 0..mags {
        for e in (0..len).rev() {
            let i = m * len + if CHAIN { e + sum } else { e };
            // SAFETY: `i` is in the array, and `sum` stays zero: the objects read zero.
            let v = unsafe { (at.add(i).read_volatile() as *const usize).read_volatile() };
            sum = sum.wrapping_add(v);
            // SAFETY: no operands but the count; nops touch nothing.
            unsafe {
                std::arch::asm!(".rept {n}", "nop", ".endr", n = const WORK, options(nostack, nomem, preserves_flags))
            };
        }
    }
    sum
}

/// [`pop_pointers`] through 32-bit offsets, which look like no pointer.
#[inline(never)]
fn pop_offsets<const WORK: usize, const CHAIN: bool>(
    at: *const u32,
    base: usize,
    line: usize,
    mags: usize,
    len: usize,
) -> usize {
    let mut sum = 0usize;
    for m in 0..mags {
        for e in (0..len).rev() {
            let i = m * len + if CHAIN { e + sum } else { e };
            // SAFETY: as in `pop_pointers`.
            let v = unsafe { ((base + at.add(i).read_volatile() as usize * line) as *const usize).read_volatile() };
            sum = sum.wrapping_add(v);
            // SAFETY: as in `pop_pointers`.
            unsafe {
                std::arch::asm!(".rept {n}", "nop", ".endr", n = const WORK, options(nostack, nomem, preserves_flags))
            };
        }
    }
    sum
}

/// Popping magazines: each holds `len` pointers to objects scattered over the heap, as magazines do
/// on a heap that has been running a while, and the program touches each object it is handed. The
/// other arm holds the same objects as 32-bit offsets. `work` instructions stand between pops, for
/// what a program does with an object. Chained, each object's value feeds the next pop; free, the
/// core overlaps what it can, as it would in a program.
fn magazine(cpu: usize, mib: usize) {
    pin(cpu);
    let cycles = Counter::cycles(cpu);
    let len_bytes = mib << 20;
    let map = Map::resident(len_bytes, Pages::Huge);
    let line = line(cpu);
    let count = (len_bytes / line).min(1 << 21);
    let mut order: Vec<usize> = (0..len_bytes / line).collect();
    Rng(0xfeed_face_cafe_1234).shuffle(&mut order);
    order.truncate(count);
    let ptrs = Map::resident(count * 8, Pages::Huge);
    let offs = Map::resident(count * 4, Pages::Huge);
    let ptr_at = ptrs.p.cast::<usize>();
    let off_at = offs.p.cast::<u32>();
    for (i, &o) in order.iter().enumerate() {
        // SAFETY: both arrays hold `count` elements.
        unsafe {
            ptr_at.add(i).write(map.p.wrapping_add(o * line) as usize);
            off_at.add(i).write(o as u32);
        }
    }
    let base = map.p as usize;
    println!(
        "# {count} scattered objects of {mib} MiB, huge pages back {} MiB, CPU {cpu}: cycles an object, highest entry first",
        huge_mib()
    );
    println!("# entries  work  chained_pointers chained_offsets  free_pointers free_offsets");
    for len in [14usize, 30, 62, 126, 254, 510, 1022] {
        let mags = count / len;
        macro_rules! row {
            ($work:literal) => {{
                let best = |f: &dyn Fn() -> usize| -> f64 {
                    let mut best = f64::MAX;
                    for _ in 0..2 {
                        let (t0, c0) = (ticks(), cycles.read());
                        std::hint::black_box(f());
                        let (c1, t1) = (cycles.read(), ticks());
                        let c = if c1 > c0 { (c1 - c0) as f64 } else { ns_of(t1 - t0) };
                        best = best.min(c / (mags * len) as f64);
                    }
                    best
                };
                println!(
                    "{len:6} {:6} {:8.1} {:8.1} {:8.1} {:8.1}",
                    $work,
                    best(&|| pop_pointers::<$work, true>(ptr_at, mags, len)),
                    best(&|| pop_offsets::<$work, true>(off_at, base, line, mags, len)),
                    best(&|| pop_pointers::<$work, false>(ptr_at, mags, len)),
                    best(&|| pop_offsets::<$work, false>(off_at, base, line, mags, len)),
                );
            }};
        }
        row!(0);
        row!(64);
        row!(256);
        row!(1024);
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let num = |i: usize, d: usize| args.get(i).and_then(|s| s.parse().ok()).unwrap_or(d);
    let cpu = num(2, 8);
    match args.get(1).map(String::as_str) {
        Some("stagger") => stagger(cpu, parse_pages(args.get(3)), num(4, 64), num(5, 1024)),
        Some("rule") => rule(cpu, parse_pages(args.get(3))),
        Some("origin") => origin(cpu),
        Some("sawtooth") => sawtooth(cpu, num(3, 512)),
        Some("pointers") => pointers(cpu, num(3, 2048)),
        Some("magazine") => magazine(cpu, num(3, 512)),
        _ => eprintln!("usage: streams stagger|rule|origin|sawtooth|pointers|magazine CPU ..."),
    }
}
