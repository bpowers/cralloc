//! Prints what a thread's rseq area holds, and whether the kernel has a registration for it,
//! at each point in a process's life where an allocator might be called:
//!
//!   ctor         an `.init_array` constructor of the executable, before `main`
//!   main         the main thread inside `main`
//!   thread       the first instruction of a new thread's start routine
//!   tls-dtor     a `thread_local!` destructor (glibc's `__call_tls_dtors`, at thread exit)
//!   key-dtor     a `pthread_key_create` destructor (later in thread exit)
//!   fork-child   a forked child
//!   vm-child     a `clone(CLONE_VM | CLONE_VFORK)` child, which is what `vfork` and glibc's
//!                `posix_spawn` create; it runs on the parent's thread pointer
//!   atexit       an `atexit` handler in the main thread
//!
//! "registered" is asked of the kernel without changing anything: an unregister request with
//! the right address and length but the wrong signature fails with EPERM when the calling
//! thread has that area registered and with EINVAL when it does not.
//!
//! Run it plain, and with `GLIBC_TUNABLES=glibc.pthread.rseq=0` to see the disabled state.
use std::arch::asm;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

extern "C" {
    static __rseq_offset: isize;
    static __rseq_size: u32;
    static __rseq_flags: u32;
    fn syscall(n: i64, ...) -> i64;
    fn __errno_location() -> *mut i32;
    fn fork() -> i32;
    fn waitpid(pid: i32, status: *mut i32, options: i32) -> i32;
    fn _exit(code: i32) -> !;
    fn clone(f: extern "C" fn(*mut u8) -> i32, stack: *mut u8, flags: i32, arg: *mut u8, ...) -> i32;
    fn mmap(addr: *mut u8, len: usize, prot: i32, flags: i32, fd: i32, off: i64) -> *mut u8;
    fn atexit(f: extern "C" fn()) -> i32;
    fn pthread_key_create(key: *mut u32, dtor: extern "C" fn(*mut u8)) -> i32;
    fn pthread_setspecific(key: u32, value: *const u8) -> i32;
    fn getauxval(t: u64) -> u64;
}

#[cfg(target_arch = "x86_64")]
const SYS_RSEQ: i64 = 334;
#[cfg(target_arch = "x86_64")]
const SYS_MEMBARRIER: i64 = 324;
#[cfg(target_arch = "aarch64")]
const SYS_RSEQ: i64 = 293;
#[cfg(target_arch = "aarch64")]
const SYS_MEMBARRIER: i64 = 283;
const MEMBARRIER_CMD_PRIVATE_EXPEDITED_RSEQ: i64 = 1 << 7;
const MEMBARRIER_CMD_REGISTER_PRIVATE_EXPEDITED_RSEQ: i64 = 1 << 8;
const RSEQ_FLAG_UNREGISTER: i64 = 1;
const CLONE_VM: i32 = 0x100;
const CLONE_VFORK: i32 = 0x4000;
const SIGCHLD: i32 = 17;
const AT_RSEQ_FEATURE_SIZE: u64 = 27;

/// A snapshot packed into one word so that any context, including a child that shares or
/// copies the address space, can publish it with a single store.
/// Bits: 0 valid, 1 registered, 8..40 cpu_id, 40..56 low 16 bits of mm_cid, 56 mm_cid >= 2^16.
fn snapshot() -> u64 {
    let tp: usize;
    #[cfg(target_arch = "x86_64")]
    unsafe {
        asm!("mov {}, fs:0", out(reg) tp, options(nostack, readonly))
    };
    #[cfg(target_arch = "aarch64")]
    unsafe {
        asm!("mrs {}, tpidr_el0", out(reg) tp, options(nomem, nostack, preserves_flags))
    };
    let area = (tp as isize + unsafe { __rseq_offset }) as *const u32;
    let len = unsafe { __rseq_size }.max(32) as i64;
    let registered = unsafe {
        syscall(SYS_RSEQ, area, len, RSEQ_FLAG_UNREGISTER, 0x1bad_c0dei64);
        *__errno_location() == 1
    };
    let (cpu_id, mm_cid) = unsafe { (area.add(1).read_volatile(), area.add(6).read_volatile()) };
    1 | (registered as u64) << 1
        | (cpu_id as u64) << 8
        | ((mm_cid & 0xffff) as u64) << 40
        | ((mm_cid > 0xffff) as u64) << 56
}

fn show(name: &str, s: u64) {
    if s & 1 == 0 {
        println!("{name:<11} did not run");
        return;
    }
    let cpu_id = (s >> 8) as u32 as i32;
    let cid = (s >> 40) & 0xffff;
    println!(
        "{name:<11} registered={:<5} cpu_id={cpu_id:<3} mm_cid={cid}{}",
        s & 2 != 0,
        if s >> 56 & 1 == 1 { " (plus high bits)" } else { "" }
    );
}

static CTOR: AtomicU64 = AtomicU64::new(0);
static TLS_DTOR: AtomicU64 = AtomicU64::new(0);
static KEY_DTOR: AtomicU64 = AtomicU64::new(0);
static VM_CHILD: AtomicU64 = AtomicU64::new(0);

extern "C" fn ctor() {
    CTOR.store(snapshot(), Relaxed);
}
#[used]
#[link_section = ".init_array"]
static CTOR_ENTRY: extern "C" fn() = ctor;

struct OnExit;
impl Drop for OnExit {
    fn drop(&mut self) {
        TLS_DTOR.store(snapshot(), Relaxed);
    }
}
thread_local!(static ON_EXIT: OnExit = const { OnExit });

extern "C" fn key_dtor(_value: *mut u8) {
    KEY_DTOR.store(snapshot(), Relaxed);
}

extern "C" fn vm_child(_arg: *mut u8) -> i32 {
    VM_CHILD.store(snapshot(), Relaxed);
    0
}

extern "C" fn at_exit() {
    show("atexit", snapshot());
}

fn main() {
    unsafe {
        println!(
            "__rseq_size={} __rseq_offset={} __rseq_flags={} AT_RSEQ_FEATURE_SIZE={}",
            __rseq_size,
            __rseq_offset,
            __rseq_flags,
            getauxval(AT_RSEQ_FEATURE_SIZE)
        );
    }
    show("ctor", CTOR.load(Relaxed));
    show("main", snapshot());

    let mut key = 0u32;
    assert_eq!(unsafe { pthread_key_create(&mut key, key_dtor) }, 0);
    let at_start = std::thread::spawn(move || {
        let s = snapshot();
        ON_EXIT.with(|_| ());
        unsafe { pthread_setspecific(key, 1 as *const u8) };
        s
    })
    .join()
    .unwrap();
    show("thread", at_start);
    show("tls-dtor", TLS_DTOR.load(Relaxed));
    show("key-dtor", KEY_DTOR.load(Relaxed));

    // PROT_READ | PROT_WRITE, MAP_SHARED | MAP_ANONYMOUS: the forked child reports through it.
    let shared = unsafe { mmap(std::ptr::null_mut(), 4096, 3, 0x21, -1, 0) } as *mut u64;
    assert!(shared as isize != -1);
    // Does a membarrier registration made before fork still count in the child?
    let registered = unsafe { syscall(SYS_MEMBARRIER, MEMBARRIER_CMD_REGISTER_PRIVATE_EXPEDITED_RSEQ, 0i64, 0i64) };
    let pid = unsafe { fork() };
    if pid == 0 {
        unsafe {
            shared.write_volatile(snapshot());
            let r = syscall(SYS_MEMBARRIER, MEMBARRIER_CMD_PRIVATE_EXPEDITED_RSEQ, 0i64, 0i64);
            shared.add(1).write_volatile(if r == 0 { 0 } else { *__errno_location() as u64 });
            _exit(0);
        }
    }
    let mut status = 0;
    unsafe { waitpid(pid, &mut status, 0) };
    show("fork-child", unsafe { shared.read_volatile() });
    println!(
        "membarrier: register in parent ret={registered}; rseq command in forked child errno={}",
        unsafe { shared.add(1).read_volatile() }
    );

    let mut stack = vec![0u8; 256 * 1024];
    let top = unsafe { stack.as_mut_ptr().add(stack.len() & !15) };
    let pid = unsafe { clone(vm_child, top, CLONE_VM | CLONE_VFORK | SIGCHLD, std::ptr::null_mut()) };
    assert!(pid > 0, "clone failed");
    unsafe { waitpid(pid, &mut status, 0) };
    show("vm-child", VM_CHILD.load(Relaxed));
    show("main again", snapshot());

    unsafe { atexit(at_exit) };
}
