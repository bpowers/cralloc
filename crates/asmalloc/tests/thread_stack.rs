//! The background thread's stack, in a program whose static thread-local data is 252,000
//! bytes: with the old fixed request of 256 KiB, glibc gave the thread about 5 KiB below that
//! data, and its first round of work ran into its guard page. The thread now asks for its
//! need plus the modules' thread-local data (`heap/background.rs`, `stack_size`).
//!
//! The test runs the thread through its paths (a burst freed, a fragmented heap, growth over
//! the cap, parking, a wake) and measures how deep its stack has been written: the stack is
//! fresh memory, zero until used, so the lowest non-zero byte below the top is the deepest
//! the thread has gone. The thread's stack pointer while it waits in a system call
//! (`/proc/self/task/<tid>/syscall`) finds its stack; the guard pages, which `/proc/self/mem`
//! cannot read, bound it. Each phase prints its depth; `docs/research/thread-stack.md` has
//! the numbers.
//!
//! It takes half a minute, so it is ignored by default; `scripts/gate --all` runs it.

use asmalloc::Asmalloc;
use std::fs::File;
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

#[global_allocator]
static ALLOCATOR: Asmalloc = Asmalloc;

/// Static thread-local data that left the old fixed request about 5 KiB of stack.
const TLS: usize = 252_000;

/// How deep below the top of its stack the thread may go in this test: the thread-local
/// data, the C library's block, and half of what the thread asks for its frames (`NEED`,
/// 128 KiB).
const DEEPEST: u64 = TLS as u64 + (8 << 10) + (64 << 10);

std::thread_local! {
    static BIG: std::cell::Cell<[u8; TLS]> = const { std::cell::Cell::new([0; TLS]) };
}

type Block = Box<[u8; 64]>;

fn burst(mib: usize) -> Vec<Block> {
    (0..mib * (1 << 20) / 64)
        .map(|i| Box::new([i as u8; 64]))
        .collect()
}

fn allocator_task() -> Option<PathBuf> {
    std::fs::read_dir("/proc/self/task").ok()?.find_map(|task| {
        let path = task.ok()?.path();
        let name = std::fs::read_to_string(path.join("comm")).ok()?;
        (name.trim() == "asmalloc").then_some(path)
    })
}

/// The thread's stack pointer, once it is waiting in a system call.
fn stack_pointer(task: &Path) -> u64 {
    loop {
        let text = std::fs::read_to_string(task.join("syscall")).unwrap();
        let fields: Vec<&str> = text.split_whitespace().collect();
        // `number args... sp pc` while in a system call; `running` otherwise.
        if fields.len() >= 9 {
            return u64::from_str_radix(fields[7].trim_start_matches("0x"), 16).unwrap();
        }
        thread::sleep(Duration::from_millis(10));
    }
}

/// The size of the thread's stack and how far below its top it has been written.
fn depth(task: &Path) -> (u64, u64) {
    let sp = stack_pointer(task);
    let maps = std::fs::read_to_string("/proc/self/maps").unwrap();
    let (lo, hi) = maps
        .lines()
        .find_map(|line| {
            let (lo, hi) = line.split_whitespace().next()?.split_once('-')?;
            let (lo, hi) = (
                u64::from_str_radix(lo, 16).ok()?,
                u64::from_str_radix(hi, 16).ok()?,
            );
            (lo <= sp && sp < hi).then_some((lo, hi))
        })
        .expect("a mapping holds the stack pointer");
    // Neighbouring stacks may share the mapping; their guard pages, which cannot be read,
    // separate them.
    let mem = File::open("/proc/self/mem").unwrap();
    let pages = ((hi - lo) / 4096) as usize;
    let mut bytes = vec![0u8; pages * 4096];
    let readable: Vec<bool> = bytes
        .chunks_mut(4096)
        .enumerate()
        .map(|(i, page)| mem.read_exact_at(page, lo + 4096 * i as u64).is_ok())
        .collect();
    let at = ((sp - lo) / 4096) as usize;
    let first = (0..=at).rev().find(|&i| !readable[i]).map_or(0, |g| g + 1);
    let end = (at..pages).find(|&i| !readable[i]).unwrap_or(pages);
    let own = &bytes[first * 4096..end * 4096];
    let lowest = own.iter().position(|&b| b != 0).unwrap() as u64;
    (own.len() as u64, own.len() as u64 - lowest)
}

fn on_a_worker(f: impl FnOnce() + Send + 'static) {
    thread::spawn(f).join().unwrap();
}

fn run() {
    // Written and read through `black_box`, so that the data is not optimized away.
    let at = std::hint::black_box(0);
    let byte = BIG.with(|big| {
        let cells = big.as_array_of_cells();
        cells[at].set(7);
        std::hint::black_box(cells)[at].get()
    });
    assert_eq!(byte, 7);
    Asmalloc::start_background().unwrap();
    let (_keep, kept) = mpsc::channel::<()>();
    thread::spawn(move || kept.recv());
    let task = allocator_task().expect("the allocator's thread");
    let mut deepest = 0;
    let mut phase = |name: &str| {
        let (size, depth) = depth(&task);
        println!("{name}: stack {size}, written {depth} below its top");
        deepest = deepest.max(depth);
    };
    phase("started");
    on_a_worker(|| drop(burst(128)));
    thread::sleep(Duration::from_secs(3));
    phase("burst");
    on_a_worker(|| {
        let kept: Vec<Block> = burst(384).into_iter().step_by(2).collect();
        std::mem::forget(kept);
    });
    thread::sleep(Duration::from_secs(3));
    phase("fragmented");
    on_a_worker(|| drop(burst(128)));
    drop(
        (0..512)
            .map(|i| vec![i as u8; 256 << 10])
            .collect::<Vec<_>>(),
    );
    drop((0..64).map(|i| vec![i as u8; 8 << 20]).collect::<Vec<_>>());
    thread::sleep(Duration::from_secs(3));
    phase("growth");
    thread::sleep(Duration::from_secs(12));
    phase("parked");
    on_a_worker(|| drop(burst(64)));
    thread::sleep(Duration::from_secs(3));
    phase("woken");
    assert!(
        deepest < DEEPEST,
        "the thread went {deepest} bytes below the top of its stack"
    );
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--list") {
        println!("thread_stack: test");
        return;
    }
    if !args
        .iter()
        .any(|a| a == "--ignored" || a == "--include-ignored")
    {
        println!("test thread_stack ... ignored, slow: run with scripts/gate --all");
        println!("\ntest result: ok. 0 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out");
        return;
    }
    run();
    println!("test thread_stack ... ok");
    println!("\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out");
}
