//! An `LD_PRELOAD` library that answers: is there ever a call to an interposed `malloc`
//! family function from a thread that has no rseq registration?
//!
//! It interposes `malloc`, `calloc`, `realloc` and `free`, forwards each to glibc's own
//! (`__libc_malloc` and friends), and on every call asks the kernel whether the calling
//! thread has glibc's rseq area registered. That covers calls from the dynamic loader, from
//! IFUNC resolvers and constructors of other libraries, from thread start and thread exit,
//! and from `vfork`/`posix_spawn` children, which run on the parent's thread pointer without
//! a registration of their own.
//!
//! It prints one line to stderr for each of the first few unregistered calls, and a summary
//! when the process exits normally. With `RSEQ_PRELOAD_QUIET=1` in the environment the
//! summary is printed only if something was unregistered.
//!
//! Build: rustc --edition 2021 -O --crate-type cdylib rseq_preload.rs -o /tmp/librseq_preload.so
//! Use:   LD_PRELOAD=/tmp/librseq_preload.so some-program
//!
//! The check is a system call per allocation, so this is a probe, not something to leave on.
//!
//! Nothing here may allocate: the functions below are the allocator.
use core::arch::asm;
use core::ffi::c_void;
use core::sync::atomic::{AtomicU64, Ordering::Relaxed};

extern "C" {
    static __rseq_offset: isize;
    static __rseq_size: u32;
    fn __libc_malloc(size: usize) -> *mut c_void;
    fn __libc_calloc(n: usize, size: usize) -> *mut c_void;
    fn __libc_realloc(p: *mut c_void, size: usize) -> *mut c_void;
    fn __libc_free(p: *mut c_void);
    fn getenv(name: *const u8) -> *const u8;
}

const SYS_WRITE: u64 = 1;
const SYS_FCNTL: u64 = 72;
const SYS_GETTID: u64 = 186;
const SYS_RSEQ: u64 = 334;
const F_DUPFD_CLOEXEC: u64 = 1030;
const EPERM: i64 = -1;

static CALLS: AtomicU64 = AtomicU64::new(0);
// A private duplicate of stderr, taken on the first call. Programs such as the coreutils
// close fd 2 in an atexit handler, which runs before this library's destructor.
static LOG_FD: AtomicU64 = AtomicU64::new(2);
static UNREGISTERED: AtomicU64 = AtomicU64::new(0);
// Registered, yet the kernel has not filled in a CPU number: should be impossible.
static NO_CPU: AtomicU64 = AtomicU64::new(0);

unsafe fn raw(n: u64, a: u64, b: u64, c: u64, d: u64) -> i64 {
    let ret: i64;
    asm!("syscall", inlateout("rax") n as i64 => ret, in("rdi") a, in("rsi") b, in("rdx") c, in("r10") d,
         lateout("rcx") _, lateout("r11") _, options(nostack));
    ret
}

struct Line {
    buf: [u8; 160],
    len: usize,
}
impl Line {
    fn text(mut self, s: &str) -> Self {
        for &b in s.as_bytes() {
            if self.len < self.buf.len() {
                self.buf[self.len] = b;
                self.len += 1;
            }
        }
        self
    }
    fn num(mut self, mut v: i64) -> Self {
        if v < 0 {
            self = self.text("-");
            v = -v;
        }
        let mut digits = [0u8; 20];
        let mut n = 0;
        loop {
            digits[n] = b'0' + (v % 10) as u8;
            n += 1;
            v /= 10;
            if v == 0 {
                break;
            }
        }
        while n > 0 {
            n -= 1;
            if self.len < self.buf.len() {
                self.buf[self.len] = digits[n];
                self.len += 1;
            }
        }
        self
    }
    fn emit(self) {
        unsafe { raw(SYS_WRITE, LOG_FD.load(Relaxed), self.buf.as_ptr() as u64, self.len as u64, 0) };
    }
}
fn line() -> Line {
    Line { buf: [0; 160], len: 0 }.text("rseq_preload: ")
}

fn check(what: &str) {
    unsafe {
        if CALLS.fetch_add(1, Relaxed) == 0 {
            let fd = raw(SYS_FCNTL, 2, F_DUPFD_CLOEXEC, 100, 0);
            if fd > 0 {
                LOG_FD.store(fd as u64, Relaxed);
            }
        }
        let tp: usize;
        asm!("mov {}, fs:0", out(reg) tp, options(nostack, readonly));
        let area = (tp as isize + __rseq_offset) as *const u32;
        let len = if __rseq_size < 32 { 32 } else { __rseq_size };
        // An unregister request with the wrong signature: EPERM if this thread has exactly
        // this area registered, EINVAL if not. Nothing changes either way.
        let registered = raw(SYS_RSEQ, area as u64, len as u64, 1, 0x1bad_c0de) == EPERM;
        let cpu_id = area.add(1).read_volatile() as i32;
        if registered {
            if cpu_id < 0 {
                NO_CPU.fetch_add(1, Relaxed);
            }
            return;
        }
        if UNREGISTERED.fetch_add(1, Relaxed) < 8 {
            line()
                .text(what)
                .text(" from a thread with no rseq registration: tid=")
                .num(raw(SYS_GETTID, 0, 0, 0, 0))
                .text(" cpu_id=")
                .num(cpu_id as i64)
                .text(" mm_cid=")
                .num(area.add(6).read_volatile() as i64)
                .text(" __rseq_size=")
                .num(__rseq_size as i64)
                .text("\n")
                .emit();
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn malloc(size: usize) -> *mut c_void {
    check("malloc");
    __libc_malloc(size)
}

#[no_mangle]
pub unsafe extern "C" fn calloc(n: usize, size: usize) -> *mut c_void {
    check("calloc");
    __libc_calloc(n, size)
}

#[no_mangle]
pub unsafe extern "C" fn realloc(p: *mut c_void, size: usize) -> *mut c_void {
    check("realloc");
    __libc_realloc(p, size)
}

#[no_mangle]
pub unsafe extern "C" fn free(p: *mut c_void) {
    check("free");
    __libc_free(p)
}

extern "C" fn summary() {
    let (calls, unregistered) = (CALLS.load(Relaxed), UNREGISTERED.load(Relaxed));
    let quiet = unsafe { !getenv(b"RSEQ_PRELOAD_QUIET\0".as_ptr()).is_null() };
    if quiet && unregistered == 0 && NO_CPU.load(Relaxed) == 0 {
        return;
    }
    line()
        .text("calls=")
        .num(calls as i64)
        .text(" unregistered=")
        .num(unregistered as i64)
        .text(" registered-without-cpu=")
        .num(NO_CPU.load(Relaxed) as i64)
        .text(" __rseq_size=")
        .num(unsafe { __rseq_size } as i64)
        .text("\n")
        .emit();
}
#[used]
#[link_section = ".fini_array"]
static SUMMARY: extern "C" fn() = summary;
