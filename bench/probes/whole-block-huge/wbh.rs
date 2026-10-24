//! Scratch switch and counters for docs/research/whole-block-huge.md, put into a copy of the
//! allocator by bench/probes/whole-block-huge/wbh_edit.py. Not product code.
//!
//! `ASMALLOC_WBH`, read once, letters each followed by digits, e.g. `m1f002d0`:
//! - `m` which large blocks allocated whole (not grown into) get huge-page advice over their
//!   whole huge pages while they hold the slot, the mark the grown path uses: 0 none (main),
//!   1 every one of at least the floor, 2 the same but only requests that are not zeroed;
//! - `f` the floor in MiB;
//! - `d` 1: count, and at exit write the counters and the map of regions to the file `WBH_DIAG`
//!   names.
//! Unset, it is main's behaviour with nothing counted.
#![allow(missing_docs, dead_code, clippy::all)]
use core::sync::atomic::{AtomicU64, Ordering::Relaxed};

unsafe extern "C" {
    fn getenv(name: *const core::ffi::c_char) -> *const core::ffi::c_char;
    fn open(path: *const core::ffi::c_char, flags: i32, mode: u32) -> i32;
    fn write(fd: i32, buf: *const u8, n: usize) -> isize;
    fn close(fd: i32) -> i32;
    fn getpid() -> i32;
}

#[derive(Clone, Copy)]
pub struct Config {
    pub mode: u8,
    pub floor: usize,
    pub diag: bool,
}

static CFG: AtomicU64 = AtomicU64::new(0);
const LOADED: u64 = 1 << 63;

#[inline]
pub fn config() -> Config {
    let mut w = CFG.load(Relaxed);
    if w & LOADED == 0 {
        w = load();
    }
    Config {
        mode: (w & 0xf) as u8,
        floor: (((w >> 8) & 0xffff) as usize) << 20,
        diag: w & (1 << 40) != 0,
    }
}

/// Whether a whole block of `size` bytes, from a zeroed request or not, gets the mark.
#[inline]
pub fn wants(size: usize, zeroed: bool) -> bool {
    let c = config();
    match c.mode {
        1 => size >= c.floor,
        2 => size >= c.floor && !zeroed,
        _ => false,
    }
}

#[cold]
fn load() -> u64 {
    let (mut mode, mut floor, mut diag) = (0u64, 2u64, 0u64);
    // SAFETY: getenv returns null or a NUL-terminated string.
    let p = unsafe { getenv(c"ASMALLOC_WBH".as_ptr()) };
    if !p.is_null() {
        let mut i = 0;
        let mut key = 0u8;
        let mut val: Option<u64> = None;
        loop {
            // SAFETY: within the NUL-terminated string.
            let c = unsafe { *p.add(i) } as u8;
            if c.is_ascii_digit() {
                val = Some(val.unwrap_or(0) * 10 + (c - b'0') as u64);
            } else {
                if let Some(v) = val.take() {
                    match key {
                        b'm' => mode = v & 0xf,
                        b'f' => floor = v.min(0xffff),
                        b'd' => diag = (v != 0) as u64,
                        _ => {}
                    }
                }
                if c == 0 {
                    break;
                }
                key = c;
            }
            i += 1;
        }
    }
    let w = LOADED | mode | floor << 8 | diag << 40;
    CFG.store(w, Relaxed);
    w
}

// --- Counters.
pub const N: usize = 32;
/// Whole blocks marked, and the bytes advised.
pub const ADV_N: usize = 0;
pub const ADV_B: usize = 1;
/// Whole blocks the arm wanted that have no whole huge page under their size.
pub const NOFIT_N: usize = 2;
/// Advice the OS refused.
pub const ADV_FAIL: usize = 3;
/// Frees of an advised slot: kept within the kept-grown budget, or returned.
pub const FREE_KEPT_N: usize = 4;
pub const FREE_RET_N: usize = 5;
pub const FREE_RET_B: usize = 6;
/// Whole blocks of at least the floor, by request: zeroed, and not.
pub const WHOLE_Z_N: usize = 7;
pub const WHOLE_NZ_N: usize = 8;
pub const WHOLE_Z_B: usize = 9;
pub const WHOLE_NZ_B: usize = 10;

pub static C: [AtomicU64; N] = [const { AtomicU64::new(0) }; N];

#[inline(always)]
pub fn add(i: usize, v: usize) {
    if v != 0 && i < N && config().diag {
        C[i].fetch_add(v as u64, Relaxed);
    }
}

/// A whole block of a huge page or more, by request.
#[inline]
pub fn count_whole(size: usize, zeroed: bool) {
    if zeroed {
        add(WHOLE_Z_N, 1);
        add(WHOLE_Z_B, size);
    } else {
        add(WHOLE_NZ_N, 1);
        add(WHOLE_NZ_B, size);
    }
}

// --- The map of regions: each assigned region's base and its class's slot size.
pub const REGIONS: usize = 8192;
static BASE: [AtomicU64; REGIONS] = [const { AtomicU64::new(0) }; REGIONS];
static SIZE: [AtomicU64; REGIONS] = [const { AtomicU64::new(0) }; REGIONS];
static REGION_BYTES: AtomicU64 = AtomicU64::new(0);

pub fn region(index: usize, base: usize, bytes: usize, class_size: usize, small: bool) {
    if index < REGIONS && config().diag {
        BASE[index].store(base as u64, Relaxed);
        SIZE[index].store(class_size as u64 | (small as u64) << 63, Relaxed);
        REGION_BYTES.store(bytes as u64, Relaxed);
    }
    // As it happens, to the file `WBH_MAP` names, for a process that never exits normally (a
    // server stopped by a signal): one line a region, `r INDEX BASE BYTES SIZE small|large`.
    // SAFETY: getenv returns null or a NUL-terminated string.
    let path = unsafe { getenv(c"WBH_MAP".as_ptr()) };
    if path.is_null() {
        return;
    }
    // SAFETY: a NUL-terminated path; O_WRONLY | O_CREAT | O_APPEND.
    let fd = unsafe { open(path, 0o1 | 0o100 | 0o2000, 0o644) };
    if fd < 0 {
        return;
    }
    let mut o = Out { fd, b: [0; 4096], n: 0 };
    o.s(b"r ");
    o.u(index as u64);
    o.s(b" ");
    o.u(base as u64);
    o.s(b" ");
    o.u(bytes as u64);
    o.s(b" ");
    o.u(class_size as u64);
    o.s(if small { b" small\n" } else { b" large\n" });
    o.flush();
    // SAFETY: our descriptor.
    unsafe {
        close(fd);
    }
}

// --- The dump at exit.
#[used]
#[unsafe(link_section = ".fini_array")]
static DUMP: extern "C" fn() = dump;

struct Out {
    fd: i32,
    b: [u8; 4096],
    n: usize,
}

impl Out {
    fn flush(&mut self) {
        // SAFETY: the buffer is live for `n` bytes.
        unsafe {
            let _ = write(self.fd, self.b.as_ptr(), self.n);
        }
        self.n = 0;
    }
    fn s(&mut self, s: &[u8]) {
        for &c in s {
            if self.n == self.b.len() {
                self.flush();
            }
            self.b[self.n] = c;
            self.n += 1;
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
        let tail = d;
        self.s(&tail[i..]);
    }
}

extern "C" fn dump() {
    let cfg = config();
    if !cfg.diag {
        return;
    }
    // SAFETY: getenv returns null or a NUL-terminated string.
    let path = unsafe { getenv(c"WBH_DIAG".as_ptr()) };
    if path.is_null() {
        return;
    }
    // SAFETY: a NUL-terminated path.
    let fd = unsafe { open(path, 0o1 | 0o100 | 0o2000, 0o644) };
    if fd < 0 {
        return;
    }
    let mut o = Out { fd, b: [0; 4096], n: 0 };
    o.s(b"wbh pid=");
    // SAFETY: getpid has no preconditions.
    o.u(unsafe { getpid() } as u64);
    o.s(b" mode=");
    o.u(cfg.mode as u64);
    o.s(b" floor=");
    o.u((cfg.floor >> 20) as u64);
    o.s(b" region_bytes=");
    o.u(REGION_BYTES.load(Relaxed));
    for (i, c) in C.iter().enumerate() {
        let v = c.load(Relaxed);
        if v != 0 {
            o.s(b" c");
            o.u(i as u64);
            o.s(b"=");
            o.u(v);
        }
    }
    o.s(b"\n");
    for i in 0..REGIONS {
        let size = SIZE[i].load(Relaxed);
        if size != 0 {
            o.s(b"r ");
            o.u(i as u64);
            o.s(b" ");
            o.u(BASE[i].load(Relaxed));
            o.s(b" ");
            o.u(size & !(1 << 63));
            o.s(if size >> 63 != 0 { b" small\n" } else { b" large\n" });
        }
    }
    o.flush();
    // SAFETY: our descriptor.
    unsafe {
        close(fd);
    }
}
