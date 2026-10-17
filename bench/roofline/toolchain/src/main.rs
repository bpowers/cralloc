//! Does a direct rip-relative load of glibc's `__rseq_offset` work in an executable, and from
//! when? Compares it with the GOT load, and checks the rseq area it names against getcpu, from a
//! .preinit_array function (the earliest user code in an executable), an .init_array function
//! and main.
use std::arch::asm;

extern "C" {
    static __rseq_offset: isize;
    fn syscall(n: i64, ...) -> i64;
}

#[inline(never)]
fn direct() -> isize {
    let v: isize;
    unsafe { asm!("mov {v}, qword ptr [rip + {s}]", v = out(reg) v, s = sym __rseq_offset, options(nostack, readonly)) };
    v
}

#[inline(never)]
fn via_got() -> isize {
    let v: isize;
    unsafe {
        asm!("mov {v}, qword ptr [rip + {s}@GOTPCREL]", "mov {v}, qword ptr [{v}]", v = out(reg) v, s = sym __rseq_offset, options(nostack, readonly))
    };
    v
}

fn cpu_from_area(off: isize) -> u32 {
    let v: u32;
    unsafe { asm!("mov {v:e}, dword ptr fs:[{o} + 4]", v = out(reg) v, o = in(reg) off, options(nostack, readonly)) };
    v
}

fn getcpu() -> u32 {
    let mut c = 0u32;
    unsafe { syscall(309, &mut c as *mut u32, 0usize, 0usize) };
    c
}

static mut EARLY: [(isize, isize, u32, u32); 2] = [(0, 0, 0, 0); 2];

extern "C" fn record(i: usize) {
    let (d, g) = (direct(), via_got());
    unsafe { EARLY[i] = (d, g, cpu_from_area(g), getcpu()) };
}
extern "C" fn pre() {
    record(0)
}
extern "C" fn init() {
    record(1)
}

#[used]
#[link_section = ".preinit_array"]
static PRE: extern "C" fn() = pre;
#[used]
#[link_section = ".init_array"]
static INIT: extern "C" fn() = init;

fn main() {
    let e = unsafe { EARLY };
    println!("preinit: direct {} got {} cpu_id(area) {} getcpu {}", e[0].0, e[0].1, e[0].2, e[0].3);
    println!("init:    direct {} got {} cpu_id(area) {} getcpu {}", e[1].0, e[1].1, e[1].2, e[1].3);
    let (d, g) = (direct(), via_got());
    println!("main:    direct {} got {} cpu_id(area via direct) {} getcpu {}", d, g, cpu_from_area(d), getcpu());
    let t = std::thread::spawn(|| {
        let d = direct();
        (d, cpu_from_area(d), getcpu())
    })
    .join()
    .unwrap();
    println!("thread:  direct {} cpu_id(area) {} getcpu {}", t.0, t.1, t.2);
}
