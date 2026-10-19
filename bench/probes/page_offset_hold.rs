//! Does a load wait for an older store at the same page offset, when the store's address
//! comes from the load?
//!
//! The loop is an inlined allocation reduced to its chain: load a class byte from a table,
//! form the slab word's address from it, load the word, load through it, store the word back.
//! The next iteration's table load follows that store. The table byte is placed at each of a
//! range of distances from the word's page offset, in a fresh 4 KiB table page and slab page
//! for every pair, so the pages differ in everything but their offsets.
//!
//! Usage: page_offset_hold [PAIRS_PER_SIDE [ITERATIONS]]. Prints, per distance, how many
//! (table page, slab page) pairs ran at each whole number of cycles per iteration. Run pinned,
//! under `scripts/measure` and `scripts/memlimit`.
use std::arch::asm;
use std::collections::BTreeMap;

unsafe extern "C" {
    fn syscall(n: i64, ...) -> i64;
    fn mmap(addr: *mut u8, len: usize, prot: i32, flags: i32, fd: i32, off: i64) -> *mut u8;
    fn madvise(addr: *mut u8, len: usize, advice: i32) -> i32;
    fn read(fd: i32, buf: *mut std::ffi::c_void, n: usize) -> isize;
}

const SYS_PERF_EVENT_OPEN: i64 = 298;
const MADV_NOHUGEPAGE: i32 = 15;
/// The slab word's page offset: the 64-byte class's hot word.
const WORD: usize = 0x110;
const DISTANCES: [isize; 17] = [-32, -16, -9, -8, -4, -1, 0, 1, 4, 7, 8, 12, 15, 16, 24, 32, 64];

/// A user-mode cycle counter for this thread.
fn cycle_counter() -> i32 {
    // struct perf_event_attr, version 8: type HARDWARE, config CPU_CYCLES, exclude_kernel and
    // exclude_hv.
    let mut attr = [0u64; 16];
    attr[0] = 128 << 32;
    attr[5] = (1 << 5) | (1 << 6);
    let fd = unsafe { syscall(SYS_PERF_EVENT_OPEN, attr.as_ptr(), 0, -1, -1, 0u64) };
    assert!(fd >= 0, "perf_event_open failed");
    fd as i32
}

fn cycles(fd: i32) -> u64 {
    let mut v = 0u64;
    assert_eq!(unsafe { read(fd, (&raw mut v).cast(), 8) }, 8);
    v
}

/// `n` zeroed 4 KiB pages, each with its own frame.
fn pages(n: usize) -> *mut u8 {
    let p = unsafe { mmap(std::ptr::null_mut(), n * 4096, 3, 0x22, -1, 0) };
    assert!(p as isize != -1, "mmap failed");
    unsafe {
        madvise(p, n * 4096, MADV_NOHUGEPAGE);
        p.write_bytes(0, n * 4096);
    }
    p
}

fn run(table_byte: *const u8, slab: *mut u8, n: u64) {
    unsafe {
        asm!(
            "2:",
            "movzx edx, byte ptr [{tab}]",
            "lea edi, [rdx * 8]",
            "or rdi, {slab}",
            "mov rcx, qword ptr [rdi]",
            "mov rax, qword ptr [rcx - 8]",
            "mov qword ptr [rdi], rcx",
            "dec {n}",
            "jnz 2b",
            tab = in(reg) table_byte,
            slab = in(reg) slab,
            n = inout(reg) n => _,
            out("rax") _, out("rcx") _, out("rdx") _, out("rdi") _,
            options(nostack),
        );
    }
}

fn main() {
    let mut args = std::env::args().skip(1).map(|a| a.parse::<u64>().expect("a number"));
    let side = args.next().unwrap_or(6) as usize;
    let iters = args.next().unwrap_or(200_000);
    let fd = cycle_counter();
    let (tables, slabs) = (pages(side), pages(side));
    let pad = Box::leak(Box::new([0u64; 64]));
    for s in 0..side {
        unsafe { slabs.add(s * 4096 + WORD).cast::<u64>().write(pad.as_ptr().add(32) as u64) };
    }
    let offset = |d: isize| (WORD as isize + d) as usize & 4095;
    for t in 0..side {
        for &d in &DISTANCES {
            unsafe { tables.add(t * 4096 + offset(d)).write((WORD / 8) as u8) };
        }
    }
    let mut seen: BTreeMap<isize, BTreeMap<u64, u32>> = BTreeMap::new();
    for t in 0..side {
        for s in 0..side {
            for &d in &DISTANCES {
                let table_byte = unsafe { tables.add(t * 4096 + offset(d)) };
                let slab = unsafe { slabs.add(s * 4096) };
                let best = (0..3)
                    .map(|_| {
                        let c0 = cycles(fd);
                        run(table_byte, slab, iters);
                        (cycles(fd) - c0) as f64 / iters as f64
                    })
                    .fold(f64::MAX, f64::min);
                *seen.entry(d).or_default().entry(best.round() as u64).or_default() += 1;
            }
        }
    }
    println!("distance from the stored word: pairs at each cycles per iteration");
    for (d, counts) in seen {
        let cells: Vec<String> = counts.iter().map(|(c, n)| format!("{c}: {n}")).collect();
        println!("{d:+4}: {}", cells.join(", "));
    }
}
