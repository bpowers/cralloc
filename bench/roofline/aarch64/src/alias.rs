//! Does a load wait on an in-flight store that shares its low address bits? Two loops, each with
//! a load of a table byte whose address shares low bits with a slab word, in a fresh pair of
//! mappings per trial:
//!
//! - `chain`: an allocation reduced to its chain. Load the byte, form the word's address from it,
//!   load the word, load through it, store the word back. The next iteration's table load follows
//!   that store. The store-to-load hop sets its pace, so a hold shorter than the hop is hidden.
//! - `store`: load the byte, form the word's address from it, store a constant there. Nothing
//!   forwards, so with no hold this runs near a cycle a pass, and any hold on the load shows.
use crate::env::map;
use crate::perf::{Counter, Pmu};

const WORD: usize = 0x118;
const DISTANCES: [isize; 13] = [-32, -16, -8, -4, -1, 0, 1, 4, 7, 8, 16, 32, 64];

extern "C" {
    fn rf_alias_chain(n: u64, table_byte: *const u8, slab: *mut u8) -> u64;
    fn rf_alias_store(n: u64, table_byte: *const u8, slab: *mut u8) -> u64;
}

std::arch::global_asm!(
    ".text",
    ".balign 64",
    ".globl rf_alias_chain",
    "rf_alias_chain:",
    ".balign 64",
    "2:",
    "ldrb w4, [x1]",
    "add x5, x2, x4, lsl #3",
    "ldr x6, [x5]",
    "ldr x7, [x6, #-8]",
    "str x6, [x5]",
    "subs x0, x0, #1",
    "b.ne 2b",
    "ret",
    ".balign 64",
    ".globl rf_alias_store",
    "rf_alias_store:",
    ".balign 64",
    "2:",
    "ldrb w4, [x1]",
    "add x5, x2, x4, lsl #3",
    "str x2, [x5]",
    "subs x0, x0, #1",
    "b.ne 2b",
    "ret",
);

type Probe = unsafe extern "C" fn(u64, *const u8, *mut u8) -> u64;

fn sweep(name: &str, f: Probe, cycles: &Counter, side: usize, iters: u64, pad: *const u64) {
    // Each span is one of the low-bit widths a load's address could be compared in, up to the
    // largest page size: 12 bits on a 4 KiB kernel, 14 on 16 KiB, 16 on 64 KiB. A table at `span`
    // from a slab, in another mapping, shares only those bits with it.
    for span_bits in [12usize, 14, 16] {
        let span = 1usize << span_bits;
        let (tables, slabs) = (map(side * 2 * span), map(side * 2 * span));
        let at = |base: *mut u8, i: usize| ((base as usize + span - 1) & !(span - 1)) + i * span;
        let byte_of = |t: usize, d: isize| at(tables, t) + ((WORD as isize + d) as usize & (span - 1));
        unsafe {
            for s in 0..side {
                ((at(slabs, s) + WORD) as *mut u64).write(pad.add(32) as u64);
            }
            for t in 0..side {
                for &d in &DISTANCES {
                    (byte_of(t, d) as *mut u8).write((WORD / 8) as u8);
                }
            }
        }
        println!("{name}, low {span_bits} bits: distance from the stored word: trials at each cycles a pass");
        for &d in &DISTANCES {
            let mut counts: Vec<(u64, u32)> = vec![];
            for t in 0..side {
                for s in 0..side {
                    let best = (0..3)
                        .map(|_| {
                            cycles.start();
                            unsafe { f(iters, byte_of(t, d) as *const u8, at(slabs, s) as *mut u8) };
                            cycles.stop().unwrap_or(u64::MAX) as f64 / iters as f64
                        })
                        .fold(f64::MAX, f64::min);
                    let tenths = (best * 10.0).round() as u64;
                    match counts.iter_mut().find(|(c, _)| *c == tenths) {
                        Some((_, n)) => *n += 1,
                        None => counts.push((tenths, 1)),
                    }
                }
            }
            counts.sort();
            let cells: Vec<String> =
                counts.iter().map(|(c, n)| format!("{:.1}: {n}", *c as f64 / 10.0)).collect();
            println!("{d:+4}: {}", cells.join(", "));
        }
    }
}

pub fn main(cpu: usize, args: &[String]) {
    let side: usize = args.first().and_then(|s| s.parse().ok()).unwrap_or(4);
    let iters: u64 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(200_000);
    let pmu = Pmu::of_cpu(cpu);
    let cycles = Counter::open(pmu.typ, pmu.cycles).expect("cycles");
    let pad = Box::leak(Box::new([0u64; 64]));
    sweep("store", rf_alias_store, &cycles, side, iters, pad.as_ptr());
    sweep("chain", rf_alias_chain, &cycles, side, iters, pad.as_ptr());
}
