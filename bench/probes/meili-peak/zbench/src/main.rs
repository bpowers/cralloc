//! The life of a zeroed large block, for docs/research/meili-peak.md: allocate it zeroed, write
//! a fraction of its pages, free it, over and over, with asmalloc as the global allocator and
//! whatever switch the environment sets (`ASMALLOC_ZPURGE` for the fixed-threshold prototype,
//! `ASMALLOC_UNWRITTEN` for the evidence rule).
//!
//!   zbench [REPS]
//!
//! For each size (1, 2, 4, 8, 16, 64 MiB), slot state and fraction written (0, 1/16, 1/8, 1/4,
//! 1/2, 1 of the pages, one store a page, evenly spread), the median cycles of one life
//! (allocate, write, free) and the minor faults a life. Slot states:
//! - `loop`: the slot the last life left, taken again at once, as a block freed and taken
//!   again within the hold;
//! - `returned`: the same, after the program returned the block's pages itself before freeing
//!   it, as a slot the hold purged in part, not known to be zero;
//! - `zero`: a slot known to be zero, after `Asmalloc::collect` returned everything;
//! - `alternate`: lives alternating between none written and all written, on one slot.
//! The program's own return and the `collect` are not timed. Then the cost of one `mincore`
//! of a single page, present and absent.
use std::alloc::{GlobalAlloc, Layout};

#[global_allocator]
static A: asmalloc::Asmalloc = asmalloc::Asmalloc;

const PAGE: usize = 4096;

fn minflt() -> i64 {
    // SAFETY: getrusage writes one rusage.
    let mut r: libc::rusage = unsafe { std::mem::zeroed() };
    unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut r) };
    r.ru_minflt
}

fn rdtsc() -> u64 {
    // SAFETY: rdtsc has no preconditions on x86-64.
    unsafe { core::arch::x86_64::_rdtsc() }
}

#[derive(Clone, Copy, PartialEq)]
enum State {
    Loop,
    Returned,
    Zero,
    Alternate,
}

/// One life: cycles timed and faults.
fn life(layout: Layout, frac: (usize, usize), state: State) -> (u64, i64) {
    let size = layout.size();
    let pages = size / PAGE;
    let written = pages * frac.0 / frac.1;
    let f0 = minflt();
    let t0 = rdtsc();
    // SAFETY: a valid layout of non-zero size.
    let p = unsafe { A.alloc_zeroed(layout) };
    assert!(!p.is_null());
    for k in 0..written {
        // Evenly spread over the block, as a sample would find it.
        let at = k * pages / written.max(1);
        // SAFETY: inside the block.
        unsafe { p.add(at * PAGE).write_volatile(1) };
    }
    let t1 = rdtsc();
    let f1 = minflt();
    if state == State::Returned {
        let start = (p as usize).next_multiple_of(PAGE);
        let end = (p as usize + size) & !(PAGE - 1);
        // SAFETY: the program's own block, whole pages of it, which it does not read again
        // before freeing it.
        unsafe { libc::madvise(start as *mut libc::c_void, end - start, libc::MADV_DONTNEED) };
    }
    let f2 = minflt();
    let t2 = rdtsc();
    // SAFETY: allocated above with this layout.
    unsafe { A.dealloc(p, layout) };
    let t3 = rdtsc();
    let f3 = minflt();
    if state == State::Zero {
        asmalloc::Asmalloc::collect();
    }
    ((t1 - t0) + (t3 - t2), (f1 - f0) + (f3 - f2))
}

fn mincore_cost() {
    let len = 64 << 20;
    // SAFETY: a fresh private anonymous mapping, unmapped below.
    let p = unsafe {
        libc::mmap(std::ptr::null_mut(), len, libc::PROT_READ | libc::PROT_WRITE, libc::MAP_PRIVATE | libc::MAP_ANONYMOUS, -1, 0)
    } as *mut u8;
    // SAFETY: inside the mapping.
    unsafe { libc::madvise(p.cast(), len, libc::MADV_NOHUGEPAGE) };
    for present in [true, false] {
        if present {
            for k in 0..len / PAGE {
                // SAFETY: inside the mapping.
                unsafe { p.add(k * PAGE).write_volatile(1) };
            }
        } else {
            // SAFETY: as above.
            unsafe { libc::madvise(p.cast(), len, libc::MADV_DONTNEED) };
        }
        let mut v = [0u8; 1];
        let mut best = u64::MAX;
        for round in 0..5 {
            let t0 = rdtsc();
            for k in 0..1024usize {
                let at = (k * 16 + round) % (len / PAGE);
                // SAFETY: one page of the mapping, one byte out.
                unsafe { libc::mincore(p.add(at * PAGE).cast(), PAGE, v.as_mut_ptr()) };
            }
            best = best.min((rdtsc() - t0) / 1024);
        }
        println!("mincore of one page, {}: {best} cycles", if present { "present" } else { "absent" });
    }
    // SAFETY: the mapping made above.
    unsafe { libc::munmap(p.cast(), len) };
}

fn main() {
    let reps: usize = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(40);
    let mode: Vec<String> = std::env::vars().filter(|(k, _)| k.starts_with("ASMALLOC_")).map(|(k, v)| format!("{k}={v}")).collect();
    println!("{}", mode.join(" "));
    println!("{:>6} {:>9} {:>6} {:>12} {:>10}", "MiB", "slot", "frac", "kcycles", "faults");
    let fracs = [(0usize, 1usize), (1, 16), (1, 8), (1, 4), (1, 2), (1, 1)];
    for mib in [1usize, 2, 4, 8, 16, 64] {
        let layout = Layout::from_size_align(mib << 20, 8).unwrap();
        for state in [State::Loop, State::Returned, State::Zero, State::Alternate] {
            let rows: &[(usize, usize)] = if state == State::Alternate { &[(0, 1), (1, 1)] } else { &fracs };
            for &frac in rows {
                let (mut cycles, mut faults) = (Vec::with_capacity(reps), 0i64);
                // Every row starts from the same heap: everything returned, so the slot is known
                // to be zero. Then two lives to settle it.
                asmalloc::Asmalloc::collect();
                for i in 0..reps + 2 {
                    let f = if state == State::Alternate {
                        // This row's fraction on odd lives, the other on even ones.
                        if i % 2 == 1 { frac } else if frac.0 == 0 { (1, 1) } else { (0, 1) }
                    } else {
                        frac
                    };
                    let (c, n) = life(layout, f, state);
                    if i >= 2 && (state != State::Alternate || i % 2 == 1) {
                        cycles.push(c);
                        faults += n;
                    }
                }
                cycles.sort_unstable();
                let name = match state {
                    State::Loop => "loop",
                    State::Returned => "returned",
                    State::Zero => "zero",
                    State::Alternate => "alternate",
                };
                println!(
                    "{:>6} {:>9} {:>6} {:>12.1} {:>10.0}",
                    mib,
                    name,
                    format!("{}/{}", frac.0, frac.1),
                    cycles[cycles.len() / 2] as f64 / 1e3,
                    faults as f64 / cycles.len() as f64
                );
            }
        }
    }
    mincore_cost();
}
