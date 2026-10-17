//! Does an `execve` that does not replace the process leave the calling thread running with
//! an out-of-range `mm_cid`?
//!
//! Reading `fs/exec.c` and `kernel/sched/core.c` in Linux 7.2.9: `bprm_execve` drops the
//! caller's concurrency ID before it tries the exec (`sched_mm_cid_before_execve`) and
//! re-adds the caller as a user when the exec does not go through
//! (`sched_mm_cid_after_execve` -> `sched_mm_cid_fork`). While the process is in per-CPU
//! ownership mode `sched_mm_cid_fork` assigns no ID, because a new task picks one up when it
//! is first scheduled in. This task is already running, and the same path calls
//! `rseq_force_update`, so on its way back to user space the kernel publishes
//! `MM_CID_UNSET` (bit 31) in the rseq area. The value stays until the thread's next context
//! switch.
//!
//! The probe holds the process in per-CPU mode with sleeping threads, makes one thread's
//! exec return in one of three ways, and reads `mm_cid` straight afterwards:
//!
//!   enoexec  an executable file no binary format accepts (ENOEXEC)
//!   shebang  a script whose interpreter does not exist (ENOENT, found after the ID was dropped)
//!   check    `execveat(..., AT_EXECVE_CHECK)` on a valid binary, which succeeds and returns
//!
//! Any value of 8192 or more is a kernel defect. An allocator that indexes a static array
//! with the cid reads or writes far outside the array.
//!
//! Usage: cid_exec_fail [threads] [attempts] [dir] [enoexec|shebang|check]
//! Run with `threads` well above 1.25 times the CPU count for per-CPU mode, and with 2 as
//! the per-task control, where the value must stay in range.
use std::arch::asm;
use std::ffi::CString;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

extern "C" {
    static __rseq_offset: isize;
    static __rseq_size: u32;
    fn syscall(n: i64, ...) -> i64;
}

const SYS_EXECVEAT: i64 = 322;
const AT_FDCWD: i64 = -100;
const AT_EXECVE_CHECK: i64 = 0x10000;
const ENOENT: i32 = 2;
const ENOEXEC: i32 = 8;

fn rseq_area() -> *const u32 {
    let tp: usize;
    unsafe { asm!("mov {}, fs:0", out(reg) tp, options(nostack, readonly)) };
    (tp as isize + unsafe { __rseq_offset }) as *const u32
}

fn main() {
    let mut args = std::env::args().skip(1);
    let threads: usize = args.next().and_then(|s| s.parse().ok()).unwrap_or(96);
    let attempts: usize = args.next().and_then(|s| s.parse().ok()).unwrap_or(20_000);
    let dir = args.next().map(Into::into).unwrap_or_else(std::env::temp_dir);
    let mode = args.next().unwrap_or_else(|| "enoexec".into());
    assert!(unsafe { __rseq_size } >= 28, "no rseq registration with mm_cid");

    let file = dir.join(format!("cid_exec_fail.{}.bin", std::process::id()));
    let (path, flags, expect): (_, i64, Option<i32>) = match mode.as_str() {
        "enoexec" => {
            std::fs::File::create(&file).unwrap().write_all(&[0u8, 1, 2, 3, 0xff, 0xfe, 0xfd, 0xfc]).unwrap();
            (file.clone(), 0, Some(ENOEXEC))
        }
        "shebang" => {
            std::fs::File::create(&file).unwrap().write_all(b"#!/nonexistent/interpreter\n").unwrap();
            (file.clone(), 0, Some(ENOENT))
        }
        "check" => ("/bin/true".into(), AT_EXECVE_CHECK, None),
        other => panic!("unknown mode {other}"),
    };
    if file.exists() {
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let cpath = CString::new(path.to_str().unwrap()).unwrap();

    let stop = Arc::new(AtomicBool::new(false));
    // Sleeping threads count as users of the address space just as running ones do, and
    // leave the CPU to the thread under test so it is rarely rescheduled before it reads.
    let sleepers: Vec<_> = (0..threads.saturating_sub(1))
        .map(|_| {
            let stop = stop.clone();
            std::thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(20));
                }
            })
        })
        .collect();
    std::thread::sleep(Duration::from_millis(100));

    let area = rseq_area();
    let argv = [cpath.as_ptr() as *const u8, std::ptr::null()];
    let envp = [std::ptr::null::<u8>()];
    // struct rseq: cpu_id_start, cpu_id, rseq_cs (u64), flags, node_id, mm_cid.
    let read_cid = || unsafe { area.add(6).read_volatile() };
    let (mut bad, mut worst, mut longest) = (0usize, 0u32, Duration::ZERO);
    let before = read_cid();
    for i in 0..attempts {
        let r = unsafe { syscall(SYS_EXECVEAT, AT_FDCWD, cpath.as_ptr(), argv.as_ptr(), envp.as_ptr(), flags) };
        let cid = read_cid();
        let errno = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
        match expect {
            Some(e) => assert!(r == -1 && errno == e, "expected errno {e}, got r={r} errno={errno}"),
            None => assert!(r == 0, "AT_EXECVE_CHECK failed: errno {errno}"),
        }
        if cid >= 8192 {
            bad += 1;
            worst = worst.max(cid);
            // How long would an allocator be exposed? Spin without entering the kernel
            // until the value is replaced, for a sample of the attempts.
            if i % 2000 == 0 {
                let t = Instant::now();
                while read_cid() >= 8192 && t.elapsed() < Duration::from_secs(2) {
                    std::hint::spin_loop();
                }
                longest = longest.max(t.elapsed());
            }
        }
    }
    stop.store(true, Ordering::Relaxed);
    for s in sleepers {
        s.join().unwrap();
    }
    let _ = std::fs::remove_file(&file);
    println!(
        "mode={mode} threads={threads} attempts={attempts} mm_cid before={before} out-of-range reads={bad} worst={worst:#x} longest observed lifetime={longest:?} (capped at 2s)"
    );
    if bad > 0 {
        println!("FAIL: the kernel published an mm_cid outside the documented range");
        std::process::exit(1);
    }
    println!("ok: mm_cid stayed in range");
}
