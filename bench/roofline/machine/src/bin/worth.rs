//! What a huge page is worth against small pages, and what getting and undoing one costs.
//!
//! ```text
//! worth churn CPU [rounds]   objects replaced at random, by heap size: fill and churn, small and huge
//! worth build CPU            a run of allocations written once, as a build phase is: per object
//! worth cold CPU             MADV_COLD as the splitter, and what DONTNEED then returns
//! worth contig CPU           one line a page on small pages, on huge pages, and on a huge page's
//!                            memory mapped by page table entries that may carry the contiguous bit
//! ```
use roofline_machine::*;

fn cgroup_mib() -> i64 {
    let read = || -> Option<i64> {
        let cg = std::fs::read_to_string("/proc/self/cgroup").ok()?;
        let path = cg.lines().next()?.strip_prefix("0::")?.to_string();
        let v: i64 = std::fs::read_to_string(format!("/sys/fs/cgroup{path}/memory.current")).ok()?.trim().parse().ok()?;
        Some(v >> 20)
    };
    read().unwrap_or(-1)
}

/// One heap of `len` bytes of `size`-byte objects: every object written, then random objects read
/// and replaced by others from a small ring of free ones, as a cache of live objects churns.
/// Returns (ns an object to fill, ns a churn operation, cycles a churn operation).
fn churn_once(cpu: usize, cycles: &Counter, len: usize, size: usize, pages: Pages) -> (f64, f64, f64) {
    let count = len / size;
    let pool = Map::new(len, pages);
    // The table the program keeps its objects in, on the same kind of page.
    let table = Map::new((count * 4).next_multiple_of(page()), pages);
    let at = table.p.cast::<u32>();
    let words = size / 8;
    let t0 = ticks();
    for i in 0..count {
        let obj = pool.p.wrapping_add(i * size).cast::<u64>();
        for w in 0..words {
            // SAFETY: inside the pool.
            unsafe { obj.add(w).write_volatile(i as u64) };
        }
        // SAFETY: the table holds `count` entries.
        unsafe { at.add(i).write_volatile(i as u32) };
    }
    let fill = ns_of(ticks() - t0) / count as f64;
    let ops = count.clamp(1 << 20, 1 << 23);
    // A ring of free objects stands for what magazines hold: the last 1024 replaced.
    let mut ring: Vec<u32> = vec![];
    for i in (0..1024.min(count / 2)).rev() {
        ring.push(i as u32);
    }
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15 ^ cpu as u64);
    let mut at_ring = 0;
    let n = ring.len();
    let mut sum = 0u64;
    let (t0, c0) = (ticks(), cycles.read());
    for _ in 0..ops {
        let slot = rng.next() as usize % count;
        // SAFETY: inside the table and the pool, by the arithmetic above.
        unsafe {
            let old = at.add(slot).read_volatile();
            let old_obj = pool.p.wrapping_add(old as usize * size).cast::<u64>();
            sum = sum.wrapping_add(old_obj.read_volatile());
            let new = std::mem::replace(&mut ring[at_ring], old);
            at_ring = (at_ring + 1) % n;
            let new_obj = pool.p.wrapping_add(new as usize * size).cast::<u64>();
            for w in 0..words {
                new_obj.add(w).write_volatile(sum);
            }
            at.add(slot).write_volatile(new);
        }
    }
    let (c1, t1) = (cycles.read(), ticks());
    std::hint::black_box(sum);
    (fill, ns_of(t1 - t0) / ops as f64, (c1 - c0) as f64 / ops as f64)
}

fn churn(cpu: usize, rounds: usize) {
    pin(cpu);
    let cycles = Counter::cycles(cpu);
    println!("# objects of 64 bytes, CPU {cpu}, page {}, huge {}: median of {rounds} interleaved rounds", page(), huge());
    println!("# MiB  fill_ns_small fill_ns_huge  churn_ns_small churn_ns_huge  churn_cyc_small churn_cyc_huge  gain");
    for mib in [4usize, 8, 16, 32, 64, 128, 256, 512, 1024, 2048, 4096] {
        if mib > max_mib(4096) {
            break;
        }
        let len = mib << 20;
        let (mut a, mut b, mut c, mut d, mut e, mut f) = (vec![], vec![], vec![], vec![], vec![], vec![]);
        for _ in 0..rounds {
            let (fill, ns, cyc) = churn_once(cpu, &cycles, len, 64, Pages::Small);
            a.push(fill);
            c.push(ns);
            e.push(cyc);
            let (fill, ns, cyc) = churn_once(cpu, &cycles, len, 64, Pages::Huge);
            b.push(fill);
            d.push(ns);
            f.push(cyc);
        }
        let (cs, ch) = (median(&mut c), median(&mut d));
        println!(
            "{mib:6} {:8.2} {:8.2}  {cs:8.2} {ch:8.2}  {:8.1} {:8.1}  {:6.3}",
            median(&mut a),
            median(&mut b),
            median(&mut e),
            median(&mut f),
            cs / ch
        );
    }
}

/// Objects of a size handed out in ascending order and written once, the faults included: what a
/// build phase pays, on each kind of page, against reusing memory already resident.
fn build(cpu: usize) {
    pin(cpu);
    let cycles = Counter::cycles(cpu);
    println!("# objects written once in ascending order, CPU {cpu}, page {}, huge {}", page(), huge());
    println!("# size  MiB  small_ns  small_cyc  huge_ns  huge_cyc  resident_ns  resident_cyc  (an object)");
    for (size, mib) in [(24usize, 256usize), (64, 256), (64, 1024), (256, 1024), (1024, 1024)] {
        if mib > max_mib(4096) {
            continue;
        }
        let len = mib << 20;
        let count = len / size;
        let words = size / 8;
        let run = |map: &Map| -> Timed {
            let (t0, c0) = (ticks(), cycles.read());
            for i in 0..count {
                let obj = map.p.wrapping_add(i * size).cast::<u64>();
                for w in 0..words {
                    // SAFETY: inside the mapping.
                    unsafe { obj.add(w).write_volatile(i as u64) };
                }
            }
            let (c1, t1) = (cycles.read(), ticks());
            Timed { cycles: (c1 - c0) as f64 / count as f64, ns: ns_of(t1 - t0) / count as f64 }
        };
        let (mut s, mut h, mut r) = (vec![], vec![], vec![]);
        for _ in 0..3 {
            let small = Map::new(len, Pages::Small);
            s.push(run(&small));
            r.push(run(&small));
            drop(small);
            let large = Map::new(len, Pages::Huge);
            h.push(run(&large));
        }
        let m = |v: &Vec<Timed>| -> Timed {
            let mut c: Vec<f64> = v.iter().map(|t| t.cycles).collect();
            let mut n: Vec<f64> = v.iter().map(|t| t.ns).collect();
            Timed { cycles: median(&mut c), ns: median(&mut n) }
        };
        let (s, h, r) = (m(&s), m(&h), m(&r));
        println!(
            "{size:6} {mib:5} {:8.2} {:8.1} {:8.2} {:8.1} {:8.2} {:8.1}",
            s.ns, s.cycles, h.ns, h.cycles, r.ns, r.cycles
        );
    }
}

fn cold(cpu: usize) {
    pin(cpu);
    let page = page();
    let huge = huge();
    let n = 32usize;
    let len = n * huge;
    let show = |what: &str| {
        println!(
            "  {what:52} rss {:6} MiB  huge {:6} MiB  cgroup {:6} MiB  thp_split_page {} thp_split_pmd {} deferred {}",
            smaps("Rss:") >> 10,
            huge_mib(),
            cgroup_mib(),
            vmstat("thp_split_page"),
            vmstat("thp_split_pmd"),
            vmstat("thp_deferred_split_page"),
        )
    };
    println!("# {n} huge pages of {} MiB, page {page}", huge >> 20);
    let timed = |map: &Map, len_at: usize, off: usize, advice: i32| -> f64 {
        let mut total = 0;
        for i in 0..n {
            let t0 = ticks();
            adv(map.p.wrapping_add(i * huge + off), len_at, advice);
            total += ticks() - t0;
        }
        ns_of(total) / 1e3 / n as f64
    };
    // A split frees the pages that hold only zero, and looks through each page to find out. So the
    // first pass writes one byte a page with a value that shows, and the last leaves them zero.
    let filled = |value: u8| {
        let m = Map::new(len, Pages::Huge);
        let mut o = 0;
        while o < len {
            // SAFETY: inside the mapping.
            unsafe { m.p.add(o + page / 2).write_volatile(value) };
            o += page;
        }
        m
    };
    let m = filled(1);
    show("filled in huge pages, a nonzero byte in the middle of each page");
    let us = timed(&m, huge, 0, MADV_COLD);
    show(&format!("MADV_COLD of each whole huge page, {us:.1} us a call"));
    let us = timed(&m, page, 0, MADV_COLD);
    show(&format!("MADV_COLD of one page of each, {us:.1} us a call"));
    let us = timed(&m, huge / 2, 0, MADV_DONTNEED);
    show(&format!("DONTNEED of the first half of each, {us:.1} us a call"));
    let us = timed(&m, huge / 2, huge / 2, MADV_DONTNEED);
    show(&format!("DONTNEED of the second half of each, {us:.1} us a call"));
    drop(m);
    let m = filled(1);
    show("filled again");
    let us = timed(&m, huge / 2, 0, MADV_COLD);
    show(&format!("MADV_COLD of the first half of each, {us:.1} us a call"));
    let us = timed(&m, huge / 2, 0, MADV_DONTNEED);
    show(&format!("DONTNEED of that half, {us:.1} us a call"));
    let us = timed(&m, huge / 2, huge / 2, MADV_DONTNEED);
    show(&format!("DONTNEED of the other half, {us:.1} us a call"));
    drop(m);
    let m = filled(1);
    show("filled again");
    let us = timed(&m, 16 * page, huge / 4, MADV_DONTNEED);
    show(&format!("DONTNEED of 16 pages inside each, {us:.1} us a call"));
    let us = timed(&m, page, 0, MADV_COLD);
    show(&format!("MADV_COLD of one page of each, after that, {us:.1} us a call"));
    // Refilling what a split left: how dear is the page after it?
    let t0 = ticks();
    for i in 0..n {
        let mut o = 0;
        while o < huge {
            // SAFETY: inside the mapping.
            unsafe { m.p.add(i * huge + o).write_volatile(2) };
            o += page;
        }
    }
    println!("  rewrite all of it after the split: {:.1} ns a page", ns_of(ticks() - t0) / (len / page) as f64);
    show("rewritten");
    drop(m);
    let m = filled(0);
    show("filled with zeros: every page of the huge pages reads zero");
    let us = timed(&m, page, 0, MADV_COLD);
    show(&format!("MADV_COLD of one page of each, {us:.1} us a call"));
}

/// The test of whether this CPU's TLB uses the contiguous bit. A partial `mprotect` splits a huge
/// page's map into page table entries but leaves its folio whole, so the kernel's contiguous-PTE
/// support may set the bit over runs of entries, which `smaps` does not show. The chase then tells:
/// if the TLB merged the runs, a walk over this memory reaches as far as the huge page's does.
fn contig(cpu: usize) {
    pin(cpu);
    let cycles = Counter::cycles(cpu);
    let page = page();
    let huge = huge();
    let span = 32 * huge;
    let small = Map::resident(span, Pages::Small);
    let whole = Map::resident(span, Pages::Huge);
    let split = Map::resident(span, Pages::Huge);
    println!("# huge pages back {} MiB before the split, of {} MiB", huge_mib(), 2 * span >> 20);
    let before = vmstat("thp_split_pmd");
    for i in 0..32 {
        let q = split.p.wrapping_add(i * huge);
        // SAFETY: one page of a live mapping, made read-only and restored.
        unsafe {
            assert_eq!(mprotect(q, page, 1), 0);
            assert_eq!(mprotect(q, page, PROT_RW), 0);
        }
    }
    println!(
        "# after the split: huge pages back {} MiB; thp_split_pmd rose {}, thp_split_page {}",
        huge_mib(),
        vmstat("thp_split_pmd") - before,
        vmstat("thp_split_page")
    );
    let line = line(cpu);
    println!("# one line a page in random order, CPU {cpu}, page {page}: cycles a load");
    println!("# pages  span_MiB  small  split  huge");
    for pages in [64usize, 128, 160, 192, 256, 512, 1024, 2048, 3072, 4096, 8192, 16384, 32768] {
        if pages * page > span - huge {
            break;
        }
        let row = |map: &Map| -> f64 {
            let mut colours = Rng(0xc0c0_1234 + pages as u64);
            // Dense pages, from past the first huge page's split run.
            let mut at: Vec<*mut u8> = (0..pages)
                .map(|i| map.p.wrapping_add(huge / 2 + i * page + (colours.next() as usize % (page / line)) * line))
                .collect();
            Rng(0x1111_2222).shuffle(&mut at);
            time_chase(link(&at), (pages * 4).clamp(1 << 18, 1 << 21), pages * 2, 3, &cycles).cycles
        };
        println!(
            "{pages:6} {:7.1} {:7.2} {:7.2} {:7.2}",
            (pages * page) as f64 / (1 << 20) as f64,
            row(&small),
            row(&split),
            row(&whole)
        );
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let num = |i: usize, d: usize| args.get(i).and_then(|s| s.parse().ok()).unwrap_or(d);
    let cpu = num(2, 8);
    match args.get(1).map(String::as_str) {
        Some("churn") => churn(cpu, num(3, 3)),
        Some("build") => build(cpu),
        Some("cold") => cold(cpu),
        Some("contig") => contig(cpu),
        _ => eprintln!("usage: worth churn|build|cold|contig CPU"),
    }
}
