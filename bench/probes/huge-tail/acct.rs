//! Counters of the tail's calls for docs/research/huge-tail.md ("The accounting"), put into a
//! scratch copy of the allocator by `acct_edit.py`, for measurement only. Each lap's end appends
//! the totals to the file `ACCT_DIAG` names, so the last line of a run is its total.
#![allow(missing_docs, dead_code, clippy::all)]
use core::sync::atomic::{AtomicU64, Ordering::Relaxed};

unsafe extern "C" {
    fn getenv(name: *const core::ffi::c_char) -> *const core::ffi::c_char;
    fn open(path: *const core::ffi::c_char, flags: i32, mode: u32) -> i32;
    fn write(fd: i32, buf: *const u8, n: usize) -> isize;
    fn close(fd: i32) -> i32;
}

/// Raises and lowerings that went through.
pub static RAISES: AtomicU64 = AtomicU64::new(0);
pub static LOWERINGS: AtomicU64 = AtomicU64::new(0);
/// Splits asked for and purges of runs in the tail, and their pages.
pub static SPLITS: AtomicU64 = AtomicU64::new(0);
pub static PURGES: AtomicU64 = AtomicU64::new(0);
pub static PURGE_PAGES: AtomicU64 = AtomicU64::new(0);
/// Purges of runs whose pages were all marked clean (made only because the bits are not
/// believed under the advice), their pages, and how many of those the kernel said were
/// resident just before.
pub static CLEAN_PURGES: AtomicU64 = AtomicU64::new(0);
pub static CLEAN_PAGES: AtomicU64 = AtomicU64::new(0);
pub static CLEAN_RESIDENT: AtomicU64 = AtomicU64::new(0);

pub fn add(counter: &AtomicU64, n: u64) {
    counter.fetch_add(n, Relaxed);
}

struct Buf {
    b: [u8; 512],
    n: usize,
}

impl Buf {
    fn s(&mut self, s: &str) {
        for &c in s.as_bytes() {
            if self.n < self.b.len() {
                self.b[self.n] = c;
                self.n += 1;
            }
        }
    }
    fn u(&mut self, mut v: u64) {
        let mut d = [0u8; 20];
        let mut i = d.len();
        loop {
            i -= 1;
            d[i] = b'0' + (v % 10) as u8;
            v /= 10;
            if v == 0 {
                break;
            }
        }
        for &c in &d[i..] {
            if self.n < self.b.len() {
                self.b[self.n] = c;
                self.n += 1;
            }
        }
    }
    fn kv(&mut self, k: &str, v: &AtomicU64) {
        self.s(" ");
        self.s(k);
        self.s("=");
        self.u(v.load(Relaxed));
    }
}

/// Appends the totals to `ACCT_DIAG`.
pub fn dump(lap: u64) {
    // SAFETY: getenv returns null or a NUL-terminated string.
    let path = unsafe { getenv(c"ACCT_DIAG".as_ptr()) };
    if path.is_null() {
        return;
    }
    let mut b = Buf { b: [0; 512], n: 0 };
    b.s("acct lap=");
    b.u(lap);
    b.kv("raises", &RAISES);
    b.kv("lowerings", &LOWERINGS);
    b.kv("splits", &SPLITS);
    b.kv("purges", &PURGES);
    b.kv("purge_pages", &PURGE_PAGES);
    b.kv("clean_purges", &CLEAN_PURGES);
    b.kv("clean_pages", &CLEAN_PAGES);
    b.kv("clean_resident", &CLEAN_RESIDENT);
    b.s("\n");
    // O_WRONLY | O_CREAT | O_APPEND | O_CLOEXEC
    // SAFETY: the path is NUL-terminated; the buffer holds `n` bytes.
    unsafe {
        let fd = open(path, 0o1 | 0o100 | 0o2000 | 0o2000000, 0o644);
        if fd >= 0 {
            let _ = write(fd, b.b.as_ptr(), b.n);
            close(fd);
        }
    }
}
