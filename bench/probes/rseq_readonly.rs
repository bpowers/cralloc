//! Does this kernel tolerate user-space writes to the rseq area's `cpu_id_start` field?
//!
//! tcmalloc caches its per-CPU slab pointer in eight bytes that overlap `cpu_id_start`, with
//! bit 63 set, and relies on the kernel rewriting the field on every reschedule to clear that
//! bit. Under a registration at the full feature size (what glibc does), 7.x kernels treat the
//! ID fields as read-only and only rewrite them when they change. This probe sets the top bit
//! and then yields, spins or migrates to show which of the two assumptions still holds.
//!
//! Usage: rseq_readonly [yield|spin|migrate]. Expect `migrate` to die with SIGSEGV on a
//! kernel that enforces the read-only fields.
use std::arch::asm;

extern "C" {
    static __rseq_offset: isize;
    fn sched_yield() -> i32;
    fn sched_setaffinity(pid: i32, len: usize, mask: *const u64) -> i32;
}

fn main() {
    let tp: usize;
    unsafe { asm!("mov {}, fs:0", out(reg) tp) };
    // struct rseq starts with { u32 cpu_id_start; u32 cpu_id; ... }.
    let rseq = (tp as isize + unsafe { __rseq_offset }) as *mut u32;
    let which = std::env::args().nth(1).unwrap_or_default();
    unsafe {
        println!("before: cpu_id_start={} cpu_id={}", rseq.read_volatile(), rseq.add(1).read_volatile());
        rseq.write_volatile(0x8000_0000 | rseq.read_volatile());
        println!("wrote high bit: cpu_id_start={:#x}", rseq.read_volatile());
        match which.as_str() {
            // Voluntary context switches without changing CPU.
            "yield" => {
                for _ in 0..1000 {
                    sched_yield();
                }
            }
            // Long enough to be preempted by the timer tick many times.
            "spin" => {
                let t = std::time::Instant::now();
                while t.elapsed().as_millis() < 300 {
                    std::hint::spin_loop();
                }
            }
            // Forces the CPU id to change, which is when the kernel next writes the field.
            // CPU numbers suit the 32-CPU reference host; any other online CPUs do.
            "migrate" => {
                for cpu in [9u64, 10, 11, 8] {
                    let m = 1u64 << cpu;
                    sched_setaffinity(0, 8, &m);
                }
            }
            _ => {}
        }
        println!("after {which}: cpu_id_start={:#x} cpu_id={}", rseq.read_volatile(), rseq.add(1).read_volatile());
    }
}
