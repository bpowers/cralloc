//! Forces an rseq abort before each instruction of a critical section, in-process and
//! without privileges.
//!
//! A perf hardware execution breakpoint with `sigtrap` set delivers SIGTRAP when the thread
//! reaches the watched address, before that instruction runs. On signal delivery the kernel
//! sees the instruction pointer inside the registered critical section and resumes at
//! `abort_ip`. Arming the breakpoint on instruction k therefore runs exactly the first k
//! instructions and then aborts: the "every prefix followed by an abort" case a restartable
//! sequence has to survive, made deterministic.
//!
//! x86-64 Linux with glibc 2.35 or later (the probe uses glibc's rseq registration).
use std::arch::asm;
use std::sync::atomic::{AtomicI32, AtomicU32, Ordering};

extern "C" {
    static __rseq_offset: isize;
    static __rseq_size: u32;
    fn syscall(n: i64, ...) -> i64;
    fn sigaction(sig: i32, act: *const SigAction, old: *mut SigAction) -> i32;
    // The linker defines `__start_<name>` for any section whose name is a C identifier.
    static __start_cs_points: [u64; 6];
}

// glibc's `struct sigaction` on x86-64.
#[repr(C)]
struct SigAction {
    handler: usize,
    mask: [u64; 16],
    flags: i32,
    restorer: usize,
}

const SYS_CLOSE: i64 = 3;
const SYS_IOCTL: i64 = 16;
const SYS_PERF_EVENT_OPEN: i64 = 298;
const SIGTRAP: i32 = 5;
const SA_SIGINFO: i32 = 4;
const PERF_EVENT_IOC_DISABLE: i64 = 0x2401;
const PERF_TYPE_BREAKPOINT: u64 = 5;
const HW_BREAKPOINT_X: u64 = 4;

static BP_FD: AtomicI32 = AtomicI32::new(-1);
static FIRED: AtomicU32 = AtomicU32::new(0);

extern "C" fn on_trap(_sig: i32, _info: *mut u8, _ctx: *mut u8) {
    FIRED.fetch_add(1, Ordering::Relaxed);
    // The retry re-executes the same address; without this it would trap forever.
    unsafe { syscall(SYS_IOCTL, BP_FD.load(Ordering::Relaxed) as i64, PERF_EVENT_IOC_DISABLE, 0i64) };
}

fn arm(addr: u64) -> i32 {
    // struct perf_event_attr as 16 u64 words (128 bytes, a size every kernel since 5.13 accepts).
    let mut attr = [0u64; 16];
    attr[0] = PERF_TYPE_BREAKPOINT | (128u64 << 32); // type, size
    attr[2] = 1; // sample_period: signal on every hit
    // exclude_kernel | exclude_hv | remove_on_exec | sigtrap. sigtrap requires remove_on_exec;
    // the two exclude bits keep the event usable at perf_event_paranoid=2.
    attr[5] = (1 << 5) | (1 << 6) | (1 << 36) | (1 << 37);
    attr[6] = HW_BREAKPOINT_X << 32; // bp_type (upper half; wakeup_events stays 0)
    attr[7] = addr; // bp_addr
    attr[8] = 8; // bp_len: execution breakpoints must use sizeof(long)
    let fd = unsafe { syscall(SYS_PERF_EVENT_OPEN, attr.as_ptr(), 0i64, -1i64, -1i64, 0i64) } as i32;
    assert!(fd >= 0, "perf_event_open failed: {}", std::io::Error::last_os_error());
    fd
}

/// The critical section under test: push `val` onto `{slots, *hdr}` with capacity `cap`.
/// Returns 0 = pushed, 1 = full, 2 = aborted by the kernel.
///
/// The store into `slots` happens before the commit, as in a real per-CPU push: an abort
/// after it must leave a dead write above the length and nothing else.
#[inline(never)]
unsafe fn push(rseq: *mut u8, hdr: *mut u64, slots: *mut u64, cap: u64, val: u64) -> u32 {
    let res: u32;
    asm!(
        // Arm: point rseq_cs (offset 8 of the rseq area) at this section's descriptor.
        "lea {tmp}, [rip + 3f]",
        "mov qword ptr [{rseq} + 8], {tmp}",
        // Labels 20..25 mark every instruction boundary inside the section. They are exported
        // through the cs_points section so the driver cannot drift from the code.
        "20:",
        "mov {tmp}, qword ptr [{hdr}]",
        "21:",
        "cmp {tmp}, {cap}",
        "22:",
        "jae 5f",
        "23:",
        "mov qword ptr [{slots} + {tmp}*8], {val}",
        "24:",
        "lea {tmp}, [{tmp} + 1]",
        "25:",
        "mov qword ptr [{hdr}], {tmp}", // the single committing store
        "4:",
        "xor {res:e}, {res:e}",
        "jmp 6f",
        "5:",
        "mov {res:e}, 1",
        "jmp 6f",
        // The kernel refuses to jump to an abort_ip that is not preceded by the signature the
        // area was registered with (glibc uses 0x53053053 on x86). The three bytes before it
        // make the pair decode as `ud1 <sig>(%rip), %edi`, the kernel selftests' convention,
        // so disassemblers stay in sync and a stray jump here traps.
        ".byte 0x0f, 0xb9, 0x3d",
        ".long 0x53053053",
        "7:",
        "mov {res:e}, 2",
        "6:",
        // struct rseq_cs { version, flags, start_ip, post_commit_offset, abort_ip }.
        ".pushsection __rseq_cs, \"aw\"",
        ".balign 32",
        "3:",
        ".long 0, 0",
        ".quad 20b, 4b - 20b, 7b",
        ".popsection",
        // "R" (retain) keeps the table through --gc-sections; nothing references it by symbol.
        ".pushsection cs_points, \"awR\"",
        ".quad 20b, 21b, 22b, 23b, 24b, 25b",
        ".popsection",
        rseq = in(reg) rseq,
        hdr = in(reg) hdr,
        slots = in(reg) slots,
        cap = in(reg) cap,
        val = in(reg) val,
        tmp = out(reg) _,
        res = out(reg) res,
        options(nostack),
    );
    res
}

fn main() {
    let act = SigAction { handler: on_trap as *const () as usize, mask: [0; 16], flags: SA_SIGINFO, restorer: 0 };
    assert_eq!(unsafe { sigaction(SIGTRAP, &act, std::ptr::null_mut()) }, 0);
    let tp: usize;
    unsafe { asm!("mov {}, fs:0", out(reg) tp) };
    let rseq = (tp as isize + unsafe { __rseq_offset }) as *mut u8;
    println!(
        "rseq area at tp{:+}, size {}, cpu_id {}",
        unsafe { __rseq_offset },
        unsafe { __rseq_size },
        unsafe { *(rseq.add(4) as *const u32) }
    );

    let points = unsafe { __start_cs_points };
    for (k, &addr) in points.iter().enumerate() {
        let mut slots = [0u64; 4];
        let mut len = 1u64;
        slots[0] = 111;
        let fd = arm(addr);
        BP_FD.store(fd, Ordering::Relaxed);
        FIRED.store(0, Ordering::Relaxed);
        let r1 = unsafe { push(rseq, &mut len, slots.as_mut_ptr(), 4, 222) };
        let (len1, s1) = (len, slots);
        let fired1 = FIRED.load(Ordering::Relaxed);
        // The handler disarmed the breakpoint, so this attempt runs to completion.
        let r2 = unsafe { push(rseq, &mut len, slots.as_mut_ptr(), 4, 222) };
        unsafe { syscall(SYS_CLOSE, fd as i64) };
        println!(
            "abort before instr {k} (ip {addr:#x}): fired={fired1} first={r1} len={len1} slots={s1:?} | retry={r2} len={len} slots={slots:?}"
        );
        assert_eq!(fired1, 1);
        assert_eq!(r1, 2, "the kernel must have redirected to abort_ip");
        assert_eq!(len1, 1, "no commit before the abort");
        assert_eq!(s1[0], 111, "live slots untouched by the aborted attempt");
        assert_eq!((r2, len, slots[1]), (0, 2, 222));
    }
    println!("all {} abort points behaved", points.len());
}
