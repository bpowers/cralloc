//! The background thread's stack, in a program whose static thread-local data is 252,000
//! bytes. With a fixed request of 256 KiB, glibc would give the thread about 5 KiB below that
//! data, and its first round of work would run into its guard page. So the thread takes the C
//! library's default stack, which the library sizes against its own thread-local data.
//!
//! The test runs the thread through its paths (a burst freed, a fragmented heap, growth over
//! the cap, parking, a wake), in the process and again in a child forked from it, and
//! measures three things (`docs/research/portability.md`, "The background thread's stack", has
//! the numbers):
//!
//! - how deep the thread's stack has been written: the stack is fresh memory, zero until
//!   used, so the lowest non-zero byte below the top is the deepest the thread has gone;
//! - how much of the stack is resident (`/proc/self/pagemap`): the touched pages only;
//! - whether the kernel keeps transparent huge pages off the stack (`nh` in its `VmFlags`),
//!   which is what keeps an 8 MiB default stack from becoming 2 MiB resident.
//!
//! The thread's stack pointer while it waits in a system call (`/proc/self/task/<tid>/syscall`)
//! finds its stack; its guard pages bound it.
//!
//! It takes about a minute, so it is ignored by default; `scripts/gate --all` runs it.

use cralloc::Cralloc;
use std::fs::File;
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

#[global_allocator]
static ALLOCATOR: Cralloc = Cralloc;

/// Static thread-local data that would leave a fixed request of 256 KiB about 5 KiB of stack.
const TLS: usize = 252_000;

/// How deep below the top of its stack the thread may go in this test: the thread-local
/// data, the C library's block, and 64 KiB for frames that reach 34 KiB in a debug build.
const DEEPEST: u64 = TLS as u64 + (8 << 10) + (64 << 10);

/// The kernel's page size.
fn page() -> u64 {
    unsafe extern "C" {
        fn sysconf(name: i32) -> i64;
    }
    const SC_PAGESIZE: i32 = 30;
    // SAFETY: a query of a constant of the system.
    unsafe { sysconf(SC_PAGESIZE) as u64 }
}

/// The C library's default stack size, guard page included.
fn default_stack_size() -> u64 {
    unsafe extern "C" {
        fn pthread_getattr_default_np(attr: *mut u64) -> i32;
        fn pthread_attr_getstacksize(attr: *const u64, size: *mut usize) -> i32;
    }
    // Room for a `pthread_attr_t` on every target.
    let mut attr = [0u64; 16];
    let mut size = 0;
    // SAFETY: `attr` has room for what the first call writes, which the second reads; `size` is
    // written. The attribute holds nothing that needs destroying when it is the default.
    unsafe {
        assert_eq!(pthread_getattr_default_np(attr.as_mut_ptr()), 0);
        assert_eq!(pthread_attr_getstacksize(attr.as_ptr(), &mut size), 0);
    }
    size as u64
}

std::thread_local! {
    static BIG: std::cell::Cell<[u8; TLS]> = const { std::cell::Cell::new([0; TLS]) };
}

unsafe extern "C" {
    fn fork() -> i32;
    fn waitpid(pid: i32, status: *mut i32, options: i32) -> i32;
    fn _exit(code: i32) -> !;
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
        (name.trim() == "cralloc").then_some(path)
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

/// The `smaps` entry of the mapping that holds `address`: its range and its `VmFlags`.
fn mapping(address: u64) -> (u64, u64, String) {
    let smaps = std::fs::read_to_string("/proc/self/smaps").unwrap();
    let mut current: Option<(u64, u64)> = None;
    for line in smaps.lines() {
        let first = line.split_whitespace().next().unwrap_or("");
        if let Some((lo, hi)) = first.split_once('-')
            && let (Ok(lo), Ok(hi)) = (u64::from_str_radix(lo, 16), u64::from_str_radix(hi, 16))
        {
            current = Some((lo, hi));
            continue;
        }
        if let (Some((lo, hi)), Some(flags)) = (current, line.strip_prefix("VmFlags:"))
            && lo <= address
            && address < hi
        {
            return (lo, hi, flags.trim().to_owned());
        }
    }
    panic!("no mapping holds {address:#x}");
}

/// What the thread's stack looks like now.
struct Stack {
    /// The thread's own stack, between the guard pages.
    size: u64,
    /// How far below its top it has been written.
    written: u64,
    /// How much of it is resident.
    resident: u64,
    flags: String,
}

fn stack(task: &Path) -> Stack {
    let sp = stack_pointer(task);
    let (lo, hi, flags) = mapping(sp);
    // Each page's entry in `/proc/self/pagemap` says whether it is present (bit 63) and
    // whether it is a guard region (bit 58, Linux 6.14), without faulting anything in.
    // Neighbouring stacks may share the mapping; their guard pages separate them.
    let pages = ((hi - lo) / page()) as usize;
    let mut entries = vec![0u8; pages * 8];
    File::open("/proc/self/pagemap")
        .unwrap()
        .read_exact_at(&mut entries, lo / page() * 8)
        .unwrap();
    let entry = |i: usize| u64::from_le_bytes(entries[i * 8..i * 8 + 8].try_into().unwrap());
    let present = |i: usize| entry(i) >> 63 & 1 != 0;
    let guard = |i: usize| entry(i) >> 58 & 1 != 0;
    let at = ((sp - lo) / page()) as usize;
    let first = (0..=at).rev().find(|&i| guard(i)).map_or(0, |g| g + 1);
    // Before Linux 6.14 no entry says guard, and a mapping with the same flags right above the stack,
    // such as the main thread's thread-local block, reads as part of it: that block was once measured
    // as 260 KiB of stack. So the stack ends where its size says, less its guard page.
    let whole = (default_stack_size() / page()) as usize - 1;
    let end = (at..pages)
        .find(|&i| guard(i))
        .unwrap_or(pages)
        .min(first + whole);
    // Only present pages are read: a read through `/proc/self/mem` maps the zero page into
    // an absent page, which would then count as present. An absent page was never written.
    let mem = File::open("/proc/self/mem").unwrap();
    let mut own = vec![0u8; (end - first) * page() as usize];
    for (i, chunk) in own.chunks_mut(page() as usize).enumerate() {
        if present(first + i) {
            mem.read_exact_at(chunk, lo + page() * (first + i) as u64)
                .unwrap();
        }
    }
    let lowest = own.iter().position(|&b| b != 0).unwrap() as u64;
    let resident = (first..end).filter(|&i| present(i)).count() as u64 * page();
    Stack {
        size: own.len() as u64,
        written: own.len() as u64 - lowest,
        resident,
        flags,
    }
}

fn on_a_worker(f: impl FnOnce() + Send + 'static) {
    thread::spawn(f).join().unwrap();
}

/// Starts the thread and runs it through its paths; `who` names the process in the output.
fn run(who: &str) {
    Cralloc::start_background().unwrap();
    let (_keep, kept) = mpsc::channel::<()>();
    thread::spawn(move || kept.recv());
    // The thread names itself as it starts.
    let started = std::time::Instant::now();
    let task = loop {
        if let Some(task) = allocator_task() {
            break task;
        }
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "{who}: no thread named cralloc"
        );
        thread::sleep(Duration::from_millis(10));
    };
    let mut last = None;
    let mut phase = |name: &str| {
        let now = stack(&task);
        println!(
            "{who} {name}: stack {}, written {} below its top, resident {}, VmFlags {}",
            now.size, now.written, now.resident, now.flags
        );
        last = Some(now);
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
    let last = last.unwrap();
    assert!(
        last.written < DEEPEST,
        "{who}: the thread went {} bytes below the top of its stack",
        last.written
    );
    // A kernel without transparent huge pages has nothing to keep off the stack, and no such flag.
    assert!(
        !Path::new("/sys/kernel/mm/transparent_hugepage").exists()
            || last.flags.split_whitespace().any(|flag| flag == "nh"),
        "{who}: the stack may take huge pages: {}",
        last.flags
    );
    // The touched pages, and a few the C library writes around them.
    let touched = last.written.next_multiple_of(page());
    assert!(
        last.resident <= touched + 4 * page(),
        "{who}: {} bytes of the stack resident, {touched} touched",
        last.resident
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
    // Written and read through `black_box`, so that the data is not optimized away.
    let at = std::hint::black_box(0);
    let byte = BIG.with(|big| {
        let cells = big.as_array_of_cells();
        cells[at].set(7);
        std::hint::black_box(cells)[at].get()
    });
    assert_eq!(byte, 7);
    run("process");
    // The same in a child forked while the parent's thread runs: the child has no thread and
    // starts its own.
    // SAFETY: the child goes on as an ordinary process and ends with `_exit`.
    let pid = unsafe { fork() };
    if pid == 0 {
        let ran = std::panic::catch_unwind(|| run("child"));
        // SAFETY: ends the child.
        unsafe { _exit(i32::from(ran.is_err())) }
    }
    let mut status = 0;
    // SAFETY: a valid pointer for the status.
    unsafe { waitpid(pid, &mut status, 0) };
    assert_eq!(status, 0, "the fork child failed");
    println!("test thread_stack ... ok");
    println!("\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out");
}
