//! System calls and `/proc` readers shared by the `remap_*` experiments (included by path from
//! each binary). The heap they build imitates asmalloc's: one `PROT_NONE`, `MAP_NORESERVE`
//! reservation cut into regions, each advised against huge pages and made writable from its
//! start, as `crates/asmalloc/src/space` and `back/region.rs` do.
#![allow(dead_code)]

use std::fs;

extern "C" {
    pub fn mmap(addr: *mut u8, len: usize, prot: i32, flags: i32, fd: i32, off: i64) -> *mut u8;
    pub fn munmap(addr: *mut u8, len: usize) -> i32;
    pub fn mremap(old: *mut u8, old_len: usize, new_len: usize, flags: i32, ...) -> *mut u8;
    pub fn madvise(addr: *mut u8, len: usize, advice: i32) -> i32;
    pub fn mprotect(addr: *mut u8, len: usize, prot: i32) -> i32;
    pub fn mlock(addr: *const u8, len: usize) -> i32;
    pub fn fork() -> i32;
    pub fn waitpid(pid: i32, status: *mut i32, options: i32) -> i32;
    pub fn _exit(code: i32) -> !;
    pub fn sched_setaffinity(pid: i32, size: usize, mask: *const u64) -> i32;
    pub fn __errno_location() -> *mut i32;
}

pub const PAGE: usize = 4096;
pub const KB: usize = 1 << 10;
pub const MB: usize = 1 << 20;
pub const GB: usize = 1 << 30;
pub const HUGE: usize = 2 * MB;

pub const NONE: i32 = 0;
pub const RW: i32 = 3;
pub const MAP_PRIVATE: i32 = 0x02;
pub const MAP_FIXED: i32 = 0x10;
pub const MAP_ANONYMOUS: i32 = 0x20;
pub const MAP_NORESERVE: i32 = 0x4000;
pub const ANON: i32 = MAP_PRIVATE | MAP_ANONYMOUS;

pub const MREMAP_MAYMOVE: i32 = 1;
pub const MREMAP_FIXED: i32 = 2;
pub const MREMAP_DONTUNMAP: i32 = 4;
/// The move under study: pages go to the new range, the old range stays mapped and empty.
pub const MOVE: i32 = MREMAP_MAYMOVE | MREMAP_FIXED | MREMAP_DONTUNMAP;

pub const MADV_DONTNEED: i32 = 4;
pub const MADV_HUGEPAGE: i32 = 14;
pub const MADV_NOHUGEPAGE: i32 = 15;
pub const MADV_COLD: i32 = 20;

pub const EFAULT: i32 = 14;
pub const ENOMEM: i32 = 12;
pub const EINVAL: i32 = 22;

pub fn errno() -> i32 {
    unsafe { *__errno_location() }
}

/// 0, or the errno of a call that returned `ret`.
pub fn check(ret: i32) -> i32 {
    if ret == 0 {
        0
    } else {
        errno()
    }
}

pub fn pin(cpu: usize) {
    let mut mask = [0u64; 16];
    mask[cpu / 64] = 1 << (cpu % 64);
    unsafe { sched_setaffinity(0, 128, mask.as_ptr()) };
}

/// A reservation like the allocator's: inaccessible, uncommitted, aligned to `align`.
pub fn reserve(len: usize, align: usize) -> *mut u8 {
    let span = len + align;
    let p = unsafe { mmap(std::ptr::null_mut(), span, NONE, ANON | MAP_NORESERVE, -1, 0) };
    assert!(p as isize != -1, "reserve: errno {}", errno());
    let start = (p as usize + align - 1) & !(align - 1);
    unsafe {
        if start > p as usize {
            munmap(p, start - p as usize);
        }
        let end = start + len;
        let tail = p as usize + span - end;
        if tail > 0 {
            munmap(end as *mut u8, tail);
        }
    }
    start as *mut u8
}

/// What the allocator does to a region of a large class before it carves from it: advise the
/// whole region against huge pages, then make a prefix writable.
pub fn prepare_region(region: *mut u8, region_len: usize, committed: usize) {
    unsafe {
        assert_eq!(madvise(region, region_len, MADV_NOHUGEPAGE), 0);
        assert_eq!(mprotect(region, committed, RW), 0);
    }
}

/// The move under study. Returns 0 or an errno.
pub fn remap(from: *mut u8, to: *mut u8, len: usize) -> i32 {
    let r = unsafe { mremap(from, len, len, MOVE, to) };
    if r as isize == -1 {
        errno()
    } else {
        assert_eq!(r, to);
        0
    }
}

/// Replaces a range's mapping with a fresh one of the heap's flags and advice, so that it can
/// merge with its neighbours again. Returns 0 or an errno.
pub fn heal(at: *mut u8, len: usize) -> i32 {
    let p = unsafe { mmap(at, len, RW, ANON | MAP_NORESERVE | MAP_FIXED, -1, 0) };
    if p as isize == -1 {
        return errno();
    }
    check(unsafe { madvise(at, len, MADV_NOHUGEPAGE) })
}

/// Whether `at..at + len` lies in one mapping, asked of the kernel without changing anything:
/// growing a range in place by a page fails with EFAULT when the range crosses the end of its
/// mapping, and with ENOMEM when it does not (there is no room to grow, the next page being
/// mapped, and no leave to move). Returns the errno.
pub fn probe(at: *mut u8, len: usize) -> i32 {
    let r = unsafe { mremap(at, len, len + PAGE, 0) };
    if r as isize == -1 {
        errno()
    } else {
        0
    }
}

pub fn touch(p: *mut u8, len: usize, step: usize, value: u8) {
    let mut o = 0;
    while o < len {
        unsafe { p.add(o).write_volatile(value) };
        o += step;
    }
}

/// Whether a page is all zero bytes.
///
/// # Safety
///
/// `page` is readable for `PAGE` bytes and aligned to 8.
#[inline(always)]
pub unsafe fn page_is_zero(page: *const u8) -> bool {
    let words = unsafe { std::slice::from_raw_parts(page as *const u64, PAGE / 8) };
    // Most pages that hold anything differ from zero early; the chunks let the all-zero case
    // vectorise.
    words.chunks_exact(8).all(|c| c.iter().fold(0, |acc, w| acc | w) == 0)
}

/// Copies `len` bytes (whole pages) to a destination that reads as zero, skipping source pages
/// that are all zero, so that a page the program never wrote costs a read fault on the shared
/// zero page and nothing at the destination. Returns the number of pages copied.
///
/// # Safety
///
/// Both ranges are valid for `len` bytes, page-aligned, and do not overlap.
pub unsafe fn copy_sparse(from: *const u8, to: *mut u8, len: usize) -> usize {
    let mut copied = 0;
    for page in 0..len / PAGE {
        unsafe {
            let src = from.add(page * PAGE);
            if !page_is_zero(src) {
                std::ptr::copy_nonoverlapping(src, to.add(page * PAGE), PAGE);
                copied += 1;
            }
        }
    }
    copied
}

/// One line of `/proc/self/maps` (or of another process's).
#[derive(Clone, Debug)]
pub struct Mapping {
    pub start: usize,
    pub end: usize,
    pub perms: String,
    pub offset: usize,
}

pub fn maps_of(pid: &str) -> Vec<Mapping> {
    let text = fs::read_to_string(format!("/proc/{pid}/maps")).unwrap();
    text.lines()
        .map(|line| {
            let mut fields = line.split_whitespace();
            let (start, end) = fields.next().unwrap().split_once('-').unwrap();
            let perms = fields.next().unwrap().to_string();
            let offset = usize::from_str_radix(fields.next().unwrap(), 16).unwrap();
            Mapping {
                start: usize::from_str_radix(start, 16).unwrap(),
                end: usize::from_str_radix(end, 16).unwrap(),
                perms,
                offset,
            }
        })
        .collect()
}

pub fn maps() -> Vec<Mapping> {
    maps_of("self")
}

/// The mappings that overlap `start..start + len`.
pub fn maps_in(start: *mut u8, len: usize) -> Vec<Mapping> {
    let (s, e) = (start as usize, start as usize + len);
    maps().into_iter().filter(|m| m.start < e && m.end > s).collect()
}

pub fn count_in(start: *mut u8, len: usize) -> usize {
    maps_in(start, len).len()
}

pub fn show(title: &str, base: *mut u8, list: &[Mapping]) {
    println!("    {title}");
    for m in list {
        println!(
            "      +{:#012x}..+{:#012x} {} {:>10} KiB",
            m.start.wrapping_sub(base as usize),
            m.end.wrapping_sub(base as usize),
            m.perms,
            (m.end - m.start) / KB
        );
    }
}

/// A field of `/proc/self/smaps_rollup` or `/proc/self/status`, in kB.
pub fn proc_kb(file: &str, key: &str) -> u64 {
    let text = fs::read_to_string(format!("/proc/self/{file}")).unwrap_or_default();
    text.lines()
        .find(|l| l.starts_with(key))
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|v| v.parse().ok())
        .unwrap_or(0)
}

pub fn rss_kb() -> u64 {
    proc_kb("status", "RssAnon:")
}

/// The `VmFlags` line and the `Rss` and `AnonHugePages` figures of the mapping holding `at`.
pub fn smaps_at(at: *mut u8) -> (String, u64, u64) {
    let text = fs::read_to_string("/proc/self/smaps").unwrap();
    let mut inside = false;
    let (mut flags, mut rss, mut huge) = (String::new(), 0, 0);
    for line in text.lines() {
        let first = line.split_whitespace().next().unwrap_or("");
        if let Some((s, e)) = first.split_once('-') {
            if let (Ok(s), Ok(e)) = (usize::from_str_radix(s, 16), usize::from_str_radix(e, 16)) {
                inside = s <= at as usize && (at as usize) < e;
                continue;
            }
        }
        if !inside {
            continue;
        }
        let value = || line.split_whitespace().nth(1).and_then(|v| v.parse().ok()).unwrap_or(0);
        if line.starts_with("Rss:") {
            rss = value();
        } else if line.starts_with("AnonHugePages:") {
            huge = value();
        } else if line.starts_with("VmFlags:") {
            flags = line["VmFlags:".len()..].trim().to_string();
        }
    }
    (flags, rss, huge)
}

/// A small deterministic generator, so runs repeat.
pub struct Rng(pub u64);

impl Rng {
    pub fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    pub fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

/// Runs `child` in a forked process and returns its exit code.
pub fn in_child(child: impl FnOnce() -> i32) -> i32 {
    unsafe {
        let pid = fork();
        assert!(pid >= 0);
        if pid == 0 {
            let code = child();
            _exit(code);
        }
        let mut status = 0;
        assert_eq!(waitpid(pid, &mut status, 0), pid);
        if status & 0x7f != 0 {
            -(status & 0x7f)
        } else {
            (status >> 8) & 0xff
        }
    }
}
