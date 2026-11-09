//! The worst case on purpose for a class's first huge page: many dense classes, each holding just
//! past a threshold, where the huge page above the watermark is up to half of what the class holds.
//!
//!   classes MIB [CLASSES]
//!
//! Allocates MIB MiB of objects in each of the first CLASSES sizes (all by default), writing each,
//! prints what the process holds, frees everything, and prints it again.
extern crate bench_alloc;

use std::alloc::{Layout, alloc, dealloc};

/// One size for each class up to 1 KiB, the classes that get huge pages.
const SIZES: [usize; 24] = [
    8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 384, 448, 512, 640, 768,
    896, 1024,
];

fn status(key: &str) -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .unwrap()
        .lines()
        .find(|l| l.starts_with(key))
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(0)
        / 1024
}

fn huge() -> u64 {
    std::fs::read_to_string("/proc/self/smaps_rollup")
        .unwrap()
        .lines()
        .find(|l| l.starts_with("AnonHugePages"))
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(0)
        / 1024
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mib: usize = args[1].parse().unwrap();
    let count: usize = args.get(2).map_or(SIZES.len(), |c| c.parse().unwrap());
    let sizes = &SIZES[..count.min(SIZES.len())];
    // Each class's objects are a list through their first words, so that what the process holds is
    // the objects and nothing kept beside them.
    let mut lists = [std::ptr::null_mut::<u8>(); SIZES.len()];
    for (list, &size) in lists.iter_mut().zip(sizes) {
        let layout = Layout::from_size_align(size, 8).unwrap();
        for _ in 0..mib * (1 << 20) / size {
            // SAFETY: a non-zero size of at least a word, written within it, and freed below.
            unsafe {
                let p = alloc(layout);
                assert!(!p.is_null());
                p.cast::<*mut u8>().write(*list);
                *list = p;
            }
        }
    }
    let asked = mib * sizes.len();
    println!(
        "classes {} each {mib} MiB, {asked} MiB in objects: rss {} hwm {} huge {} MiB",
        sizes.len(),
        status("VmRSS"),
        status("VmHWM"),
        huge()
    );
    for (list, &size) in lists.iter().zip(sizes) {
        let layout = Layout::from_size_align(size, 8).unwrap();
        let mut at = *list;
        while !at.is_null() {
            // SAFETY: allocated above with this layout, with the next object in its first word.
            unsafe {
                let next = at.cast::<*mut u8>().read();
                dealloc(at, layout);
                at = next;
            }
        }
    }
    println!("after free: rss {} huge {} MiB", status("VmRSS"), huge());
}
