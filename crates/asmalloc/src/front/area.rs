//! The calling thread's rseq area on musl, which registers none, so the crate brings its own.
//!
//! The area and the word holding its offset from the thread pointer are generated data
//! (`generated/x86_64/area.rs`, from `VG.X86_64.ownRseqArea` in Lean), and the routines read
//! that word as glibc's `__rseq_offset` (`super::__rseq_offset`). Every thread's copy of the
//! area starts with `cpu_id` -1, which the routines read as "no slab", so a new thread takes the
//! slow path, whose first question, `Real::cpu`, calls [`register`] when it reads -1. From then
//! on `cpu_id` is the thread's CPU and the fast path runs.
//!
//! **What registering lazily cannot protect.** A routine arms its sequence (points `rseq_cs` at
//! its descriptor) and then reads `cpu_id`. On a thread still at -1 the kernel aborts nothing,
//! so a signal between those two instructions is delivered with the routine's arm in place. If
//! the handler allocates, and that is the thread's first allocator call, its slow path registers
//! the thread, and the handler's own sequences leave `rseq_cs` naming one of theirs. The
//! interrupted routine then reads a valid CPU number and runs to its commit with nothing to
//! restart it: a second preemption, migration or signal within its few instructions would let
//! another thread interleave on that CPU's slab. The routines' contracts require the thread to
//! be registered, or unregistered, for the whole call, and this breaks that; no fix fits inside
//! lazy registration, so the design states it under "What is trusted". It needs a handler that
//! allocates as the thread's first allocator call, at exactly that boundary, and then a second
//! event within about eight instructions.
//!
//! `cpu_id` is also all the state the area has, so the thread keeps nothing else:
//!
//! - -1: never tried;
//! - a CPU number: registered;
//! - -2 ([`RETIRED`]): refused by the kernel (seccomp, or another area already registered), or
//!   unregistered as the thread ends. Never registered again; the slow path serves the thread
//!   without rseq, as it serves a thread whose registration glibc failed.
//!
//! **Thread exit.** musl frees a detached thread's thread-local data, the area with it, just
//! before the thread ends (it unmaps the thread's stack mapping, then exits), and the kernel
//! writes a registered area on the way back to user space from that very `munmap`: a thread
//! that dies registered can take the process with it (SIGSEGV), or write into memory mapped
//! there since. So a registered thread unregisters before its data is freed, from the
//! destructor of a pthread key, which musl runs from `pthread_exit` before it frees anything.
//! The key is made once per process, by the first registration; each registered thread gives
//! it a value, its area's address, so that the destructor runs for it. Destructors of other
//! keys may run after ours and free memory: they find `cpu_id` -2 and take the slow path. A
//! thread that never registers gives the key no value and runs nothing at exit. The main thread
//! ends with the process, which frees nothing first.
//!
//! **Limits.**
//!
//! - A thread whose first allocator call comes after our destructor can no longer run (from
//!   another key's destructor in musl's last round, `PTHREAD_DESTRUCTOR_ITERATIONS`, or after
//!   the destructors) registers and dies registered; if it is detached, that can crash the
//!   process, as above.
//! - Destructors that run after ours, which in practice include Rust's thread-local
//!   destructors, run without rseq, on the slow path: a thread that exits holding many blocks in
//!   thread-locals frees them more slowly than on glibc.
//! - A seccomp filter that fails `arch_prctl(ARCH_GET_FS)` leaves the thread at -1, so every
//!   slow path tries again (one failed system call each); one that kills the process on
//!   `arch_prctl` or `rseq` kills it at the thread's first slow path.
//! - The first registration of the process makes the key, and `pthread_key_create` takes musl's
//!   key lock: a signal handler that allocates for the first time in the process while the same
//!   thread holds that lock (inside `pthread_key_create` or `pthread_key_delete`) deadlocks.
//!
//! These are, with `__errno_location`, the crate's only calls into the C library; the ledger
//! entry for the unsafe blocks here is `docs/safety/asmalloc/front/area-safety.md`.

use crate::os::linux::rseq;
use crate::spec;
use core::ffi::{c_int, c_void};
use core::ptr::NonNull;
use core::sync::atomic::{AtomicI32, AtomicU32, Ordering};

/// What `cpu_id` holds once the thread will never register: the kernel's
/// `RSEQ_CPU_ID_REGISTRATION_FAILED`, which glibc writes there too.
pub(crate) const RETIRED: i32 = -2;

/// `cpu_id` before the thread has tried: -1, as the generated area starts.
pub(crate) const UNREGISTERED: u32 = spec::RSEQ_CPU_ID_UNREGISTERED;

// The area's page offset is pinned, the same in every thread (the thread pointer is aligned to
// the area's section, a page). It is 32-byte aligned, as every registered area is, so `cpu_id`
// and `rseq_cs` are at bytes 4 to 15 of a 32-byte chunk, where no slab hot word is (`slab`): a
// load of `cpu_id` at the page offset of a hot word stored just before would wait for that
// store, two cycles a pair (docs/research/musl-and-static.md). It is also below the words the C
// routines read at page offsets from 1024 up.
const _: () = {
    let start = spec::RSEQ_AREA_PAGE_OFFSET;
    let end = start + spec::RSEQ_AREA_LEN;
    assert!(spec::RSEQ_AREA_ALIGN == spec::SLAB_BYTES);
    assert!(start.is_multiple_of(32) && end <= 1024);
    assert!(
        spec::RSEQ_CPU_ID + 4 <= spec::RSEQ_AREA_LEN && spec::RSEQ_CS + 8 <= spec::RSEQ_AREA_LEN
    );
};

/// The key whose destructor unregisters a thread's area, plus one; zero until made.
static KEY: AtomicU32 = AtomicU32::new(0);

// SAFETY: AREA-01.
#[link(name = "c")]
unsafe extern "C" {
    fn pthread_key_create(
        key: *mut u32,
        destructor: Option<unsafe extern "C" fn(*mut c_void)>,
    ) -> c_int;
    fn pthread_key_delete(key: u32) -> c_int;
    fn pthread_setspecific(key: u32, value: *const c_void) -> c_int;
}

/// The calling thread's area.
fn area() -> Option<NonNull<u8>> {
    let tp = rseq::thread_pointer()?;
    NonNull::new(tp.as_ptr().wrapping_byte_offset(super::__rseq_offset))
}

/// The area's `cpu_id`, which the kernel writes while the area is registered.
///
/// # Safety
///
/// `area` is the calling thread's area (from [`area`], or its value of the key).
unsafe fn cpu_id<'a>(area: NonNull<u8>) -> &'a AtomicI32 {
    // SAFETY: AREA-02.
    unsafe { AtomicI32::from_ptr(area.byte_add(spec::RSEQ_CPU_ID).cast().as_ptr()) }
}

/// Registers the calling thread's area, which reads -1: the thread has never tried. Afterwards
/// it reads the thread's CPU, or [`RETIRED`] if the kernel refused.
#[cold]
#[inline(never)]
pub(crate) fn register() {
    let Some(area) = area() else { return };
    let Some(key) = key() else {
        // SAFETY: AREA-02.
        unsafe { retire(area) };
        return;
    };
    // The value comes first, so that the thread never runs registered without it.
    // SAFETY: AREA-03.
    unsafe { pthread_setspecific(key, area.as_ptr().cast()) };
    // SAFETY: AREA-04.
    match unsafe { rseq::register(area, spec::RSEQ_AREA_LEN, spec::RSEQ_SIGNATURE) } {
        // EBUSY: a signal handler that interrupted this thread since it read -1 registered it.
        Ok(()) | Err(rseq::EBUSY) => {}
        // SAFETY: AREA-02.
        Err(_) => unsafe { retire(area) },
    }
}

/// Marks `area` as never to be registered again.
///
/// # Safety
///
/// `area` is the calling thread's, and not registered.
unsafe fn retire(area: NonNull<u8>) {
    // SAFETY: the caller's promise.
    unsafe { cpu_id(area) }.store(RETIRED, Ordering::Relaxed);
}

/// The key, made by the first call of the process; `None` if the C library refuses one.
fn key() -> Option<u32> {
    let made = KEY.load(Ordering::Acquire);
    if made != 0 {
        return Some(made - 1);
    }
    let mut key = 0u32;
    // SAFETY: AREA-03.
    if unsafe { pthread_key_create(&mut key, Some(unregister_at_exit)) } != 0 {
        return None;
    }
    match KEY.compare_exchange(0, key + 1, Ordering::AcqRel, Ordering::Acquire) {
        Ok(_) => Some(key),
        Err(made) => {
            // Another thread made one first; ours has no values yet.
            // SAFETY: AREA-03.
            unsafe { pthread_key_delete(key) };
            Some(made - 1)
        }
    }
}

/// The key's destructor: unregisters the exiting thread's area, `value`, and retires it, with
/// signals blocked so that no handler finds the area unregistered and not yet retired, and
/// registers it again with no destructor left to run.
///
/// # Safety
///
/// Called by the C library, as the destructor of [`KEY`], on the thread whose area `value` is.
// The contract musl calls it under is AREA-05.
unsafe extern "C" fn unregister_at_exit(value: *mut c_void) {
    let Some(area) = NonNull::new(value.cast::<u8>()) else {
        return;
    };
    let mask = rseq::block_signals();
    if rseq::unregister(area, spec::RSEQ_AREA_LEN, spec::RSEQ_SIGNATURE).is_ok() {
        // SAFETY: AREA-02; the area is unregistered now.
        unsafe { retire(area) };
    }
    if let Some(mask) = mask {
        rseq::restore_signals(mask);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Asmalloc;
    use crate::front::{Real, Routines};
    use core::alloc::{GlobalAlloc, Layout};
    use std::sync::Arc;
    use std::sync::atomic::AtomicUsize;
    use std::vec::Vec;

    unsafe extern "C" {
        fn fork() -> i32;
        fn waitpid(pid: i32, status: *mut i32, options: i32) -> i32;
        fn _exit(code: i32) -> !;
        fn prctl(option: c_int, ...) -> c_int;
    }

    /// The calling thread's `cpu_id`, read without registering.
    fn read_cpu_id() -> i32 {
        // SAFETY: the calling thread's own area.
        unsafe { cpu_id(area().unwrap()) }.load(Ordering::Relaxed)
    }

    /// Allocates and frees a few hundred blocks through the process heap, checking contents.
    fn churn_blocks(seed: u64) {
        let layout = Layout::from_size_align(48, 8).unwrap();
        let mut held = Vec::new();
        for i in 0..400u64 {
            // SAFETY: a non-zero size; each block is freed once below, with its layout.
            let p = unsafe { Asmalloc.alloc(layout) }.cast::<u64>();
            assert!(!p.is_null());
            // SAFETY: the block holds 48 bytes.
            unsafe { p.write(seed ^ i) };
            held.push((p, seed ^ i));
            if held.len() > 64 {
                let (p, v) = held.swap_remove((i as usize * 7) % held.len());
                // SAFETY: a live block of `layout` that this thread wrote.
                unsafe {
                    assert_eq!(p.read(), v);
                    Asmalloc.dealloc(p.cast(), layout);
                }
            }
        }
        for (p, v) in held {
            // SAFETY: as above.
            unsafe {
                assert_eq!(p.read(), v);
                Asmalloc.dealloc(p.cast(), layout);
            }
        }
    }

    const CHILD: &str = "ASMALLOC_AREA_CHILD";
    const CHILD_DEADLINE: std::time::Duration = std::time::Duration::from_secs(120);

    /// Runs the test `name` of this module again in a fresh process, in which it does its work
    /// (`CHILD` is set), and fails if that process fails or outlives `CHILD_DEADLINE`. A fresh
    /// process, not a fork: a child forked from the test harness's threads may inherit a lock
    /// of std's held by one of them, and the first thread it starts then waits for it for ever.
    fn in_fresh_process(name: &str) {
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                &std::format!("front::area::tests::{name}"),
                "--nocapture",
            ])
            .env(CHILD, name)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let started = std::time::Instant::now();
        while child.try_wait().unwrap().is_none() {
            if started.elapsed() > CHILD_DEADLINE {
                child.kill().unwrap();
                panic!("{name} hung: {:?}", child.wait_with_output().unwrap());
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success(), "{name} failed: {output:?}");
        let stdout = std::string::String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains("1 passed"),
            "{name} did not run in the child: {stdout}"
        );
    }

    /// Whether this process is the fresh one `in_fresh_process` started for the test `name`.
    fn is_child(name: &str) -> bool {
        std::env::var_os(CHILD).is_some_and(|v| v == name)
    }

    /// Runs `child` in a forked child, which must start no thread, and returns its exit code.
    fn in_child(child: fn() -> i32) -> i32 {
        // SAFETY: the child runs `child` and exits without returning into the test harness.
        unsafe {
            let pid = fork();
            if pid == 0 {
                _exit(child());
            }
            let mut status = 0;
            assert_eq!(waitpid(pid, &mut status, 0), pid);
            assert_eq!(
                status & 0x7f,
                0,
                "the child ended by signal {}",
                status & 0x7f
            );
            (status >> 8) & 0xff
        }
    }

    #[test]
    fn a_new_thread_registers_at_its_first_slow_path() {
        let (before, cpu, after) = std::thread::spawn(|| {
            let before = read_cpu_id();
            let cpu = Real.cpu();
            (before, cpu, read_cpu_id())
        })
        .join()
        .unwrap();
        assert_eq!(before, -1, "the area starts unregistered");
        assert!(cpu.is_some(), "registered: the thread's CPU has a slab");
        assert!(after >= 0);
    }

    #[test]
    fn the_area_sits_at_its_page_offset_in_every_thread() {
        let here = area().unwrap().addr().get() % spec::RSEQ_AREA_ALIGN;
        let there = std::thread::spawn(|| area().unwrap().addr().get() % spec::RSEQ_AREA_ALIGN)
            .join()
            .unwrap();
        assert_eq!(here, spec::RSEQ_AREA_PAGE_OFFSET);
        assert_eq!(there, spec::RSEQ_AREA_PAGE_OFFSET);
    }

    /// musl unmaps a detached thread's stack and thread-local data before the thread ends; a
    /// thread that died registered would make the kernel write freed memory (a SIGSEGV that ends
    /// the process, in our measurements within a thousand such threads).
    #[test]
    fn detached_threads_come_and_go_registered() {
        let name = "detached_threads_come_and_go_registered";
        if !is_child(name) {
            return in_fresh_process(name);
        }
        let live = Arc::new(AtomicUsize::new(0));
        for i in 0..3000u64 {
            while live.load(Ordering::Relaxed) > 32 {
                std::thread::yield_now();
            }
            live.fetch_add(1, Ordering::Relaxed);
            let live = live.clone();
            // Dropping the handle detaches the thread.
            drop(std::thread::spawn(move || {
                churn_blocks(i);
                assert!(read_cpu_id() >= 0);
                live.fetch_sub(1, Ordering::Relaxed);
            }));
        }
        while live.load(Ordering::Relaxed) > 0 {
            std::thread::yield_now();
        }
    }

    static SEEN_AT_EXIT: AtomicI32 = AtomicI32::new(0);

    /// The destructor of a key made after ours, which musl runs after ours: it frees a block of
    /// the heap, as a thread-local's destructor would.
    unsafe extern "C" fn free_at_exit(block: *mut c_void) {
        SEEN_AT_EXIT.store(read_cpu_id(), Ordering::Relaxed);
        let layout = Layout::from_size_align(48, 8).unwrap();
        // SAFETY: the block the thread allocated with `layout` and gave the key.
        unsafe {
            assert_eq!(block.cast::<u64>().read(), 0x5eed);
            Asmalloc.dealloc(block.cast(), layout);
        }
        churn_blocks(7);
    }

    #[test]
    fn destructors_after_ours_run_on_the_slow_path() {
        let ours = key().unwrap();
        let mut theirs = 0;
        // SAFETY: a destructor that frees what the thread gave the key.
        let made = unsafe { pthread_key_create(&mut theirs, Some(free_at_exit)) };
        assert_eq!(made, 0);
        assert!(theirs > ours, "musl runs the destructors in key order");
        std::thread::spawn(move || {
            let layout = Layout::from_size_align(48, 8).unwrap();
            // SAFETY: a non-zero size; the destructor frees it.
            let block = unsafe { Asmalloc.alloc(layout) }.cast::<u64>();
            // SAFETY: the block holds 48 bytes.
            unsafe { block.write(0x5eed) };
            assert!(read_cpu_id() >= 0);
            // SAFETY: a key this test made.
            unsafe { pthread_setspecific(theirs, block.cast()) };
        })
        .join()
        .unwrap();
        assert_eq!(SEEN_AT_EXIT.load(Ordering::Relaxed), RETIRED);
        // SAFETY: the key is this test's, and no thread has a value for it any more.
        unsafe { pthread_key_delete(theirs) };
    }

    #[test]
    fn a_thread_that_never_allocates_leaves_nothing_to_do() {
        let cpu = std::thread::spawn(read_cpu_id).join().unwrap();
        assert_eq!(cpu, -1);
    }

    /// A seccomp filter that makes `rseq` fail with EPERM, as a sandbox might.
    fn refuse_rseq() {
        #[repr(C)]
        struct Filter(u16, u8, u8, u32);
        #[repr(C)]
        struct Program(u16, *const Filter);
        const LOAD_NR: u16 = 0x20; // BPF_LD | BPF_W | BPF_ABS, offset 0: the call's number
        const JUMP_EQ: u16 = 0x15; // BPF_JMP | BPF_JEQ | BPF_K
        const RET: u16 = 0x06; // BPF_RET | BPF_K
        let filter = [
            Filter(LOAD_NR, 0, 0, 0),
            Filter(JUMP_EQ, 0, 1, 334),
            Filter(RET, 0, 0, 0x0005_0000 | 1), // SECCOMP_RET_ERRNO | EPERM
            Filter(RET, 0, 0, 0x7fff_0000),     // SECCOMP_RET_ALLOW
        ];
        let program = Program(4, filter.as_ptr());
        // SAFETY: PR_SET_NO_NEW_PRIVS, then PR_SET_SECCOMP with SECCOMP_MODE_FILTER and a live
        // program; this process is a child that exits after the test.
        unsafe {
            assert_eq!(prctl(38, 1u64, 0u64, 0u64, 0u64), 0);
            assert_eq!(prctl(22, 2u64, &raw const program, 0u64, 0u64), 0);
        }
    }

    #[test]
    fn a_refused_registration_takes_the_slow_path() {
        let name = "a_refused_registration_takes_the_slow_path";
        if !is_child(name) {
            return in_fresh_process(name);
        }
        refuse_rseq();
        let threads: Vec<_> = (0..4)
            .map(|i| {
                std::thread::spawn(move || {
                    churn_blocks(i);
                    (Real.cpu().is_none(), read_cpu_id())
                })
            })
            .collect();
        for t in threads {
            assert_eq!(t.join().unwrap(), (true, RETIRED));
        }
    }

    #[test]
    fn a_child_forked_from_a_threaded_parent_keeps_its_registration() {
        assert!(Real.cpu().is_some());
        let others: Vec<_> = (0..4)
            .map(|i| std::thread::spawn(move || (0..20).for_each(|j| churn_blocks(i * 100 + j))))
            .collect();
        // The child starts no thread (see `in_fresh_process`); it allocates on the thread that
        // forked, which keeps its registration and area.
        let exit = in_child(|| {
            if read_cpu_id() < 0 || Real.cpu().is_none() {
                return 1;
            }
            churn_blocks(1);
            if read_cpu_id() < 0 { 2 } else { 0 }
        });
        for t in others {
            t.join().unwrap();
        }
        assert_eq!(exit, 0);
    }
}
