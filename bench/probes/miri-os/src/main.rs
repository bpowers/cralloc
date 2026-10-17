//! Which OS-facing operations does Miri accept from a custom global allocator?
//!
//! The default run shows that a `#[global_allocator]` backed by `mmap` works under Miri. Each
//! argument then tries one operation an allocator back end would want, so the ones Miri
//! rejects are known and can be kept behind the OS-backend trait's Miri implementation.
//!
//! Usage: cargo +nightly miri run -- [madvise|partial_munmap|mprotect|prot_none|noreserve|asm|thread]
use std::alloc::{GlobalAlloc, Layout};
use std::sync::atomic::{AtomicPtr, AtomicUsize, Ordering};

extern "C" {
    fn mmap(addr: *mut u8, len: usize, prot: i32, flags: i32, fd: i32, off: i64) -> *mut u8;
    fn munmap(addr: *mut u8, len: usize) -> i32;
    fn madvise(addr: *mut u8, len: usize, advice: i32) -> i32;
    fn mprotect(addr: *mut u8, len: usize, prot: i32) -> i32;
}

const RW: i32 = 3;
const PRIVATE_ANON: i32 = 0x22;
const MAP_NORESERVE: i32 = 0x4000;
const MADV_DONTNEED: i32 = 4;

// A bump allocator is enough: the question is what Miri lets the allocator ask of the OS,
// not what the allocator does with the memory. Not thread-safe beyond what the probe needs.
struct Bump {
    base: AtomicPtr<u8>,
    off: AtomicUsize,
}

unsafe impl GlobalAlloc for Bump {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let mut base = self.base.load(Ordering::Relaxed);
        if base.is_null() {
            base = mmap(std::ptr::null_mut(), 1 << 20, RW, PRIVATE_ANON, -1, 0);
            self.base.store(base, Ordering::Relaxed);
        }
        let o = (self.off.load(Ordering::Relaxed) + l.align() - 1) & !(l.align() - 1);
        self.off.store(o + l.size(), Ordering::Relaxed);
        base.add(o)
    }
    unsafe fn dealloc(&self, _p: *mut u8, _l: Layout) {}
}

#[global_allocator]
static A: Bump = Bump { base: AtomicPtr::new(std::ptr::null_mut()), off: AtomicUsize::new(0) };

fn main() {
    let which = std::env::args().nth(1).unwrap_or_default();
    let v: Vec<u64> = (0..100).collect();
    println!("global allocator over mmap works: sum={}", v.iter().sum::<u64>());
    unsafe {
        let p = mmap(std::ptr::null_mut(), 1 << 16, RW, PRIVATE_ANON, -1, 0);
        match which.as_str() {
            "madvise" => println!("madvise(DONTNEED) = {}", madvise(p, 4096, MADV_DONTNEED)),
            "partial_munmap" => println!("partial munmap = {}", munmap(p.add(4096), 4096)),
            "mprotect" => println!("mprotect = {}", mprotect(p, 4096, 0)),
            "prot_none" => {
                println!("mmap PROT_NONE = {:?}", mmap(std::ptr::null_mut(), 1 << 16, 0, PRIVATE_ANON, -1, 0))
            }
            "noreserve" => println!(
                "mmap NORESERVE = {:?}",
                mmap(std::ptr::null_mut(), 1 << 16, RW, PRIVATE_ANON | MAP_NORESERVE, -1, 0)
            ),
            "asm" => {
                let x: u64;
                std::arch::asm!("mov {}, fs:0", out(reg) x);
                println!("asm {x}");
            }
            "thread" => {
                let t = std::thread::spawn(|| Box::new(5u64));
                println!("thread box {}", t.join().unwrap());
            }
            _ => {}
        }
    }
}
