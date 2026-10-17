//! Fast-path roofline microbenchmarks: hand-written x86-64 kernels timed with hardware counters.
//!
//!   roofline-kernels [prefix[,prefix...]]
//!
//! runs every kernel whose name starts with one of the prefixes (all of them with no argument)
//! and prints cycles, instructions, retired uops and store-to-load forwards per operation.
//! RF_CPU picks the CPU to pin to (default 2), RF_ITERS the iteration count (default 40M).
//! `roofline-kernels routines ...` measures the proved slab routines instead (see routines.rs).
mod kernels;
mod layout;
mod routines;
use kernels::*;
use std::time::Instant;

extern "C" {
    fn syscall(num: i64, ...) -> i64;
    fn ioctl(fd: i32, req: u64, ...) -> i32;
    fn read(fd: i32, buf: *mut core::ffi::c_void, n: usize) -> isize;
    fn mmap(addr: *mut u8, len: usize, prot: i32, flags: i32, fd: i32, off: i64) -> *mut u8;
    fn sched_setaffinity(pid: i32, sz: usize, mask: *const u64) -> i32;
    static __rseq_offset: isize;
    static __rseq_size: u32;
    static __rseq_flags: u32;
}

struct Counter {
    fd: i32,
}

impl Counter {
    fn new(typ: u32, config: u64) -> Option<Counter> {
        let mut attr = [0u64; 16];
        attr[0] = (typ as u64) | (128u64 << 32);
        attr[1] = config;
        // disabled | exclude_kernel | exclude_hv
        attr[5] = 1 | (1 << 5) | (1 << 6);
        let fd = unsafe { syscall(298, attr.as_ptr(), 0i32, -1i32, -1i32, 0u64) };
        if fd < 0 {
            None
        } else {
            Some(Counter { fd: fd as i32 })
        }
    }
    fn start(&self) {
        unsafe {
            ioctl(self.fd, 0x2403, 0);
            ioctl(self.fd, 0x2400, 0);
        }
    }
    fn stop(&self) -> u64 {
        let mut v = 0u64;
        unsafe {
            ioctl(self.fd, 0x2401, 0);
            read(self.fd, &mut v as *mut u64 as *mut core::ffi::c_void, 8);
        }
        v
    }
}

fn map(len: usize) -> *mut u8 {
    // PROT_READ|PROT_WRITE, MAP_PRIVATE|MAP_ANONYMOUS
    let p = unsafe { mmap(std::ptr::null_mut(), len, 3, 0x22, -1, 0) };
    assert!(p as isize != -1);
    p
}

fn map_aligned(len: usize, align: usize) -> *mut u8 {
    let p = map(len + align) as usize;
    ((p + align - 1) & !(align - 1)) as *mut u8
}

unsafe fn w64(p: *mut u8, off: usize, v: u64) {
    (p.add(off) as *mut u64).write(v)
}

const OBJ: usize = 64;
const NCID: usize = 64;

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Simple,
    Fl,
    Flh,
    Arr,
    Bm,
    Tcm,
    Rq,
    Mi,
}

struct Env {
    tls: *mut u8,
    state: *mut u8,
    heap: *mut u8,
    arena: *mut u8,
    mags: *mut u8,
    slabs: *mut u8,
    pcpu: *mut u8,
    page: *mut u8,
    arr: *mut u8,
    rseq_off: u64,
    tpoff: u64,
}

impl Env {
    unsafe fn obj(&self, i: usize) -> u64 {
        self.arena.add(i * OBJ) as u64
    }

    unsafe fn link(&self, first: usize, n: usize) -> u64 {
        for i in 0..n {
            let next = if i + 1 < n { self.obj(first + i + 1) } else { 0 };
            w64(self.arena, (first + i) * OBJ, next);
        }
        self.obj(first)
    }

    unsafe fn fill_mag(&self, mag: *mut u8, first: usize, n: usize) -> u64 {
        for i in 0..n {
            w64(mag, 8 * (1 + i), self.obj(first + i));
        }
        mag.add(8 * (1 + n)) as u64
    }

    unsafe fn prepare(&self, kind: Kind) -> (*mut u8, u64, u64) {
        std::ptr::write_bytes(self.tls, 0, 1024);
        std::ptr::write_bytes(self.state, 0, 4096);
        std::ptr::write_bytes(self.heap, 0, 4096);
        w64(self.tls, 64, self.heap as u64);
        match kind {
            Kind::Simple => {
                w64(self.state, 0, self.state as u64);
                w64(self.state, 16, self.state as u64 + 64);
                (self.state, self.tpoff, 0)
            }
            Kind::Fl => {
                w64(self.tls, 0, self.link(0, 48));
                (self.state, 0, 0)
            }
            Kind::Flh => {
                w64(self.heap, 128, self.link(0, 48));
                (self.state, 0, 128)
            }
            Kind::Arr => {
                let top = self.fill_mag(self.mags, 0, 40);
                w64(self.tls, 8, top);
                w64(self.tls, 16, self.mags as u64 + 8);
                w64(self.tls, 24, self.mags as u64 + 512);
                (self.state, 0, 0)
            }
            Kind::Bm => {
                w64(self.tls, 48, !0u64);
                w64(self.tls, 56, self.arena as u64);
                (self.state, 0, 0)
            }
            Kind::Tcm => {
                std::ptr::write_bytes(self.slabs, 0, 65536);
                let class = 3usize;
                let begin = 64usize;
                let n = 40usize;
                w64(self.slabs, 8 * (begin - 1), 1);
                for i in 0..n {
                    w64(self.slabs, 8 * (begin + i), self.obj(i));
                }
                let hdr = ((begin + n) as u32) | (100u32 << 16);
                (self.slabs.add(4 * class) as *mut u32).write(hdr);
                w64(self.tls, 72, self.slabs as u64 | (1 << 63));
                (self.state, self.rseq_off, class as u64)
            }
            Kind::Rq => {
                let class_off = 64usize;
                for c in 0..2 * NCID {
                    // The same layout is written to the mapped slab and to the static one.
                    let region = if c < NCID { self.pcpu.add(c * 4096) } else { (&raw mut rf_slab as *mut u8).add((c - NCID) * 4096) };
                    let c = c % NCID;
                    let top = self.fill_mag(self.mags.add(c * 512), c * 64, 40);
                    w64(region, class_off, top);
                    // The intrusive-list variant uses the upper 24 objects of the same slice.
                    w64(region, class_off + 2048, self.link(c * 64, 48));
                    // Index-addressed variant: count at +1024, slots at +1088.
                    for i in 0..40 {
                        w64(region, 1088 + 8 * i, self.obj(c * 64 + i));
                    }
                    (region.add(class_off + 1024) as *mut u32).write(40);
                }
                rf_static[2] = self.rseq_off;
                rf_static[3] = self.pcpu as u64;
                (self.pcpu, self.rseq_off, class_off as u64)
            }
            Kind::Mi => {
                let class_off = 256usize;
                w64(self.heap, class_off, self.page as u64);
                w64(self.page, 0, self.link(0, 48));
                w64(self.page, 8, 0);
                w64(self.page, 16, 1000);
                w64(self.page, 24, rf_fs0());
                (self.state, 0, class_off as u64)
            }
        }
    }
}

struct Spec {
    name: &'static str,
    f: Kernel,
    kind: Kind,
    ops: u64,
    note: &'static str,
}

fn specs() -> Vec<Spec> {
    macro_rules! s {
        ($f:ident, $kind:expr, $ops:expr, $note:expr) => {
            Spec { name: stringify!($f), f: $f, kind: $kind, ops: $ops, note: $note }
        };
    }
    use Kind::*;
    vec![
        s!(k_reg, Simple, 1, "add reg (1-cycle reference)"),
        s!(k_lduse, Simple, 1, "pointer chase [rax]"),
        s!(k_lduse_idx, Simple, 1, "pointer chase [rax+rcx*8]"),
        s!(k_q_base, Simple, 1, "ld/add/st 64-bit [reg+16]"),
        s!(k_d_base, Simple, 1, "ld/add/st 32-bit [reg+16]"),
        s!(k_w_base, Simple, 1, "ld/add/st 16-bit [reg+16]"),
        s!(k_b_base, Simple, 1, "ld/add/st 8-bit [reg+16]"),
        s!(k_q_base0, Simple, 1, "ld/add/st 64-bit [reg]"),
        s!(k_q_rsp, Simple, 1, "ld/add/st 64-bit [rsp+16]"),
        s!(k_w_rsp, Simple, 1, "ld/add/st 16-bit [rsp+16]"),
        s!(k_q_rip, Simple, 1, "ld/add/st 64-bit [rip+sym]"),
        s!(k_w_rip, Simple, 1, "ld/add/st 16-bit [rip+sym]"),
        s!(k_q_fs, Simple, 1, "ld/add/st 64-bit fs:[disp]"),
        s!(k_d_fs, Simple, 1, "ld/add/st 32-bit fs:[disp]"),
        s!(k_w_fs, Simple, 1, "ld/add/st 16-bit fs:[disp]"),
        s!(k_b_fs, Simple, 1, "ld/add/st 8-bit fs:[disp]"),
        s!(k_q_fsreg, Simple, 1, "ld/add/st 64-bit fs:[reg+128]"),
        s!(k_q_idx, Simple, 1, "ld/add/st 64-bit [reg+idx*8+16]"),
        s!(k_q_rmw, Simple, 1, "add qword [reg+16], 1"),
        s!(k_d_rmw, Simple, 1, "add dword [reg+16], 1"),
        s!(k_w_rmw, Simple, 1, "add word [reg+16], 1"),
        s!(k_q_rmw_fs, Simple, 1, "add qword fs:[disp], 1"),
        s!(k_mix_s32_l16, Simple, 1, "store 32, load 16, add"),
        s!(k_mix_s64_l32, Simple, 1, "store 64, load 32, add"),
        s!(k_q_alias, Simple, 1, "64-bit, load via rsi, store via r8"),
        s!(k_q_rebase, Simple, 1, "64-bit, base reg copied each iter"),
        s!(k_q_ldbase, Simple, 1, "64-bit, base reg loaded from TLS each iter"),
        s!(k_q_between, Simple, 1, "64-bit, unrelated store in between"),
        s!(k_cmpxchg, Simple, 1, "load + lock cmpxchg"),
        s!(k_xadd, Simple, 1, "lock xadd"),
        s!(k_xchg, Simple, 1, "xchg mem"),
        s!(k_lockadd, Simple, 1, "lock add mem, 1"),
        s!(k_lockbts, Simple, 1, "lock bts mem, 5"),
        s!(k_treiber, Simple, 1, "Treiber pop+push (2 CAS)"),
        s!(k_mpsc_push, Simple, 1, "MPSC push (1 xchg + 1 store)"),
        s!(k_call, Simple, 1, "call + ret"),
        s!(k_call2, Simple, 1, "2x (call + ret)"),
        s!(k_icall, Simple, 1, "indirect call + ret"),
        s!(k_alu5, Simple, 1, "5 adds + dec/jnz"),
        s!(k_alu7, Simple, 1, "7 adds + dec/jnz"),
        s!(k_alu11, Simple, 1, "7 adds + 4 lea + dec/jnz"),
        s!(k_ld4, Simple, 1, "4 loads"),
        s!(k_ld6, Simple, 1, "6 loads"),
        s!(k_ld8, Simple, 1, "8 loads"),
        s!(k_st2, Simple, 1, "2 stores"),
        s!(k_st4, Simple, 1, "4 stores"),
        s!(k_st6, Simple, 1, "6 stores"),
        s!(k_mix8, Simple, 1, "2 loads + 2 stores + 3 adds"),
        s!(k_br2, Simple, 1, "2 taken branches"),
        s!(k_br3, Simple, 1, "3 taken branches"),
        s!(k_rseq_arm, Simple, 1, "store to rseq_cs"),
        s!(k_rseq_cid, Rq, 1, "mm_cid -> region -> load"),
        s!(p_fl, Fl, 1, "pair: TLS intrusive list"),
        s!(p_flh, Flh, 1, "pair: TLS heap ptr -> intrusive list"),
        s!(p_arr, Arr, 1, "pair: TLS magazine, low-bit bounds"),
        s!(p_arrb, Arr, 1, "pair: TLS array, bounds loaded"),
        s!(p_bm, Bm, 1, "pair: TLS bitmap word"),
        s!(p_mi, Mi, 1, "pair: mimalloc-shaped"),
        s!(p_tcm, Tcm, 1, "pair: tcmalloc rseq slab (exact, with prefetch)"),
        s!(p_tcmnp, Tcm, 1, "pair: tcmalloc rseq slab, no prefetch"),
        s!(p_rqa, Rq, 1, "pair: rseq per-CPU magazine (mm_cid)"),
        s!(p_rqg, Rq, 1, "pair: same, globals reloaded per op"),
        s!(p_rqf, Rq, 1, "pair: rseq per-CPU intrusive list"),
        s!(c_fl, Fl, 1, "call pair: TLS intrusive list"),
        s!(c_arr, Arr, 1, "call pair: TLS magazine"),
        s!(c_mi, Mi, 1, "call pair: mimalloc-shaped"),
        s!(c_tcm, Tcm, 1, "call pair: tcmalloc rseq slab"),
        s!(c_rqa, Rq, 1, "call pair: rseq per-CPU magazine"),
        s!(c_rqg, Rq, 1, "call pair: same, globals reloaded"),
        s!(b_fl, Fl, 32, "batch 32: TLS intrusive list"),
        s!(b_flh, Flh, 32, "batch 32: TLS heap ptr -> list"),
        s!(b_arr, Arr, 32, "batch 32: TLS magazine"),
        s!(b_bm, Bm, 32, "batch 32: TLS bitmap word"),
        s!(b_mi, Mi, 32, "batch 32: mimalloc-shaped"),
        s!(b_tcm, Tcm, 32, "batch 32: tcmalloc rseq slab"),
        s!(b_tcmnp, Tcm, 32, "batch 32: tcmalloc, no prefetch"),
        s!(b_rqa, Rq, 32, "batch 32: rseq per-CPU magazine"),
        s!(b_rqg, Rq, 32, "batch 32: same, globals reloaded"),
        s!(b_rqf, Rq, 32, "batch 32: rseq per-CPU intrusive list"),
        s!(k_q_abs, Simple, 1, "ld/add/st 64-bit ds:[abs32]"),
        s!(k_q_fs0base, Simple, 1, "ld/add/st 64-bit via r=fs:[0]; [r+tpoff]"),
        s!(k_w_mov16, Simple, 1, "ld/add/st 16-bit, mov ax (no movzx)"),
        s!(k_lock2, Simple, 1, "2 lock adds, different lines"),
        s!(k_work16, Simple, 1, "16 dependent adds"),
        s!(k_lock_work, Simple, 1, "lock add + 16 dependent adds"),
        s!(k_lock_ldchain, Simple, 1, "lock add + 4 chained loads"),
        s!(k_work8, Simple, 1, "8 dependent adds"),
        s!(k_call_work, Simple, 1, "call/ret + 8 dependent adds"),
        s!(m1_st_after, Simple, 1, "chain + unrelated store after commit"),
        s!(m9_two_st_after, Simple, 1, "chain + 2 unrelated stores after"),
        s!(m10_st_before, Simple, 1, "unrelated store, then chain"),
        s!(m11_idx, Simple, 1, "indexed chain + unrelated store"),
        s!(m2_fsst_after, Simple, 1, "chain + fs:[reg] store after commit"),
        s!(m3_fsld, Simple, 1, "unused fs:[reg] load, then chain"),
        s!(m4_base_fsld, Simple, 1, "base = fs load, shl, add"),
        s!(m5_base_alu, Simple, 1, "base = reg, shl, add"),
        s!(m6_base_add, Simple, 1, "base = lea [rsi+r10]"),
        s!(m7_base_ld, Simple, 1, "base = plain load"),
        s!(m8_base_ldadd, Simple, 1, "base = load, add"),
        s!(p_e1, Rq, 1, "magazine pair + { lea r10, [rip + rf_static] }"),
        s!(p_e2, Rq, 1, "magazine pair + { mov qword ptr [rbx + 512], rdi }"),
        s!(p_e3, Rq, 1, "magazine pair + { mov qword ptr fs:[rdx + 8], 0 }"),
        s!(p_e4, Rq, 1, "magazine pair + { mov r10d, dword ptr fs:[rdx + 24] }"),
        s!(p_e5, Rq, 1, "magazine pair + { mov r8, rbx }"),
        s!(p_e7, Rq, 1, "magazine pair + { nop;  nop;  nop }"),
        s!(p_e8, Rq, 1, "magazine pair + { xor r11d, r11d;  lea r8, [rbx + r11] }"),
        s!(p_e9, Rq, 1, "magazine pair + { mov r8, qword ptr [rip + rf_static + 24] }"),
        s!(p_e10, Rq, 1, "magazine pair + { mov r8d, dword ptr fs:[rdx + 24];  add r8, rbx }"),
        s!(p_e11, Rq, 1, "magazine pair + { mov r8, qword ptr [rip + rf_static + 24];  add r8, 0 }"),
        s!(p_e12, Rq, 1, "magazine pair + { lea r10, [rip + rf_static];  mov qword ptr [rbx + 512], r10 }"),
        s!(p_rqc, Rq, 1, "pair: rseq magazine, static slab, const class"),
        s!(p_rqd, Rq, 1, "pair: rseq magazine, static slab, class in reg"),
        s!(c_rqc, Rq, 1, "call pair: rseq magazine, static slab, const class"),
        s!(c_rqd, Rq, 1, "call pair: rseq magazine, static slab, class in reg"),
        s!(b_rqc, Rq, 32, "batch 32: rseq magazine, static slab, const class"),
        s!(b_rqd, Rq, 32, "batch 32: rseq magazine, static slab, class in reg"),
        s!(p_fl2, Fl, 1, "pair: TLS list via base reg"),
        s!(p_arr2, Arr, 1, "pair: TLS magazine via base reg"),
        s!(p_bm2, Bm, 1, "pair: TLS bitmap via base reg"),
        s!(p_rq0, Rq, 1, "pair: bare magazine, base hoisted"),
        s!(p_rqh, Rq, 1, "pair: magazine + rseq arm, no cid"),
        s!(p_rqn, Rq, 1, "pair: magazine + cid, no rseq arm"),
        s!(p_rqi, Rq, 1, "pair: rseq per-CPU, 32-bit index"),
        s!(c_fl2, Fl, 1, "call pair: TLS list via base reg"),
        s!(c_arr2, Arr, 1, "call pair: TLS magazine via base reg"),
        s!(c_rqf, Rq, 1, "call pair: rseq per-CPU intrusive list"),
        s!(b_fl2, Fl, 32, "batch 32: TLS list via base reg"),
        s!(b_arr2, Arr, 32, "batch 32: TLS magazine via base reg"),
        s!(b_bm2, Bm, 32, "batch 32: TLS bitmap via base reg"),
        s!(b_rq0, Rq, 32, "batch 32: bare magazine"),
        s!(b_rqi, Rq, 32, "batch 32: rseq per-CPU, 32-bit index"),
    ]
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let cpu: usize = std::env::var("RF_CPU").ok().and_then(|s| s.parse().ok()).unwrap_or(2);
    let filter = args.get(1).cloned().unwrap_or_default();
    let mask = [1u64 << cpu, 0, 0, 0];
    unsafe { sched_setaffinity(0, 32, mask.as_ptr()) };

    let (rseq_off, rseq_size, rseq_flags) = unsafe { (__rseq_offset, __rseq_size, __rseq_flags) };
    let tls = unsafe { rf_tls_addr() };
    let fs0 = unsafe { rf_fs0() };
    let rseq = (fs0 as isize + rseq_off) as *const u32;
    let (cpu_id, kflags, node, cid, slice) = unsafe {
        (rseq.add(1).read_volatile(), rseq.add(4).read_volatile(), rseq.add(5).read_volatile(),
         rseq.add(6).read_volatile(), rseq.add(7).read_volatile())
    };
    println!(
        "rseq: offset {rseq_off} size {rseq_size} glibc-flags {rseq_flags:#x}; cpu_id {cpu_id} node {node} mm_cid {cid} kernel-flags {kflags:#x} slice_ctrl {slice:#x}"
    );

    let env = Env {
        tls,
        state: map(1 << 16),
        heap: map(1 << 16),
        arena: map_aligned(1 << 20, 1 << 16),
        mags: map_aligned(NCID * 512, 4096),
        slabs: map_aligned(1 << 16, 1 << 16),
        pcpu: map_aligned(NCID * 4096, 4096),
        page: map(4096),
        arr: map(4096),
        rseq_off: rseq_off as u64,
        tpoff: (tls as u64).wrapping_sub(fs0),
    };

    // Page map for the mimalloc-shaped kernel: top level indexed by addr >> 29, sub level by
    // (addr >> 16) & 0x1fff. The top level is sparse and stays mostly untouched.
    unsafe {
        let top = map(8 << 19);
        let sub = map(8 << 13);
        let a = env.arena as usize;
        for slice in 0..16usize {
            let p = a + (slice << 16);
            w64(top, 8 * (p >> 29), sub as u64);
            w64(sub, 8 * ((p >> 16) & 0x1fff), env.page as u64);
        }
        rf_static[1] = top as u64;
    }

    unsafe {
        // MAP_FIXED_NOREPLACE | MAP_PRIVATE | MAP_ANONYMOUS
        let p = mmap(0x2000_0000 as *mut u8, 4096, 3, 0x100022, -1, 0);
        assert_eq!(p as usize, 0x2000_0000);
    }
    let cycles = Counter::new(0, 0).expect("perf_event_open cycles");
    let instrs = Counter::new(0, 1).expect("perf_event_open instructions");
    let uops = Counter::new(4, 0xC1);
    let stlf = Counter::new(4, 0x35);

    let iters: u64 = std::env::var("RF_ITERS").ok().and_then(|s| s.parse().ok()).unwrap_or(40_000_000);
    if filter == "routines" {
        let ctx = routines::Ctx {
            cycles,
            instrs,
            uops,
            mags: map_aligned(4096, 4096),
            arena: env.arena,
            arr: env.arr,
            rseq,
            iters,
        };
        return routines::main(&ctx, &args[2..]);
    }
    println!("{:<14} {:>8} {:>8} {:>8} {:>8} {:>8}  {}", "kernel", "cyc/op", "ins/op", "uops/op", "stlf/op", "ns/op", "what");
    let mut all = specs();
    for (name, f, note) in layout::specs() {
        all.push(Spec { name, f, kind: Kind::Rq, ops: 1, note });
    }
    for s in all {
        if !filter.is_empty() && !filter.split(',').any(|f| s.name.starts_with(f)) {
            continue;
        }
        let n = (iters / s.ops).max(1);
        let mut best = (f64::MAX, 0.0, 0.0, 0.0, 0.0);
        for rep in 0..4 {
            let (state, aux, class) = unsafe { env.prepare(s.kind) };
            let aborts0 = unsafe { rf_abort_count };
            cycles.start();
            instrs.start();
            if let Some(u) = &uops { u.start() }
            if let Some(u) = &stlf { u.start() }
            let t = Instant::now();
            unsafe { (s.f)(n, state, aux, env.arr, class) };
            let ns = t.elapsed().as_nanos() as f64;
            let sf = stlf.as_ref().map(|u| u.stop()).unwrap_or(0);
            let u = uops.as_ref().map(|u| u.stop()).unwrap_or(0);
            let i = instrs.stop();
            let c = cycles.stop();
            let aborts = unsafe { rf_abort_count } - aborts0;
            let d = (n * s.ops) as f64;
            // The first repetition warms up; an rseq abort in a short run is noise worth knowing about.
            if rep > 0 && (c as f64 / d) < best.0 {
                best = (c as f64 / d, i as f64 / d, u as f64 / d, sf as f64 / d, ns / d);
            }
            if aborts > 0 && rep == 3 {
                eprintln!("  ({} rseq aborts in last rep of {})", aborts, s.name);
            }
        }
        println!(
            "{:<14} {:>8.2} {:>8.2} {:>8.2} {:>8.2} {:>8.3}  {}",
            s.name, best.0, best.1, best.2, best.3, best.4, s.note
        );
    }
}
