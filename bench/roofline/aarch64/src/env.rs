//! What a kernel runs on: state blocks, the slabs and their magazines, thread and rseq facts.
use std::ptr::null_mut;

extern "C" {
    fn mmap(addr: *mut u8, len: usize, prot: i32, flags: i32, fd: i32, off: i64) -> *mut u8;
    fn sched_setaffinity(pid: i32, sz: usize, mask: *const u64) -> i32;
    static __rseq_offset: isize;
    static __rseq_size: u32;
}

pub type Kernel = unsafe extern "C" fn(u64, *mut u8, u64, *mut u8, u64) -> u64;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Kind {
    /// A zeroed state block in x20; the kernel's own setup writes what it needs.
    Simple,
    /// The static slabs and tables prepared on this CPU: x20 the CPU's slab, x21 the rseq offset,
    /// x22 an array for batches, x23 the hot word's offset of the constant class.
    Slab,
    /// A list in place of the constant class's magazine: x20 as for `Slab`, the word holds the
    /// head.
    List,
    /// tcmalloc's slab layout: x20 the slab, x23 the class index.
    Tcm,
}

pub struct Env {
    pub cpu: usize,
    pub state: *mut u8,
    pub arr: *mut u8,
    pub arena: *mut u8,
    pub mags: *mut u8,
    pub rseq_off: u64,
}

pub fn pin(cpu: usize) {
    let mask = [1u64 << cpu, 0, 0, 0];
    assert_eq!(unsafe { sched_setaffinity(0, 32, mask.as_ptr()) }, 0);
}

pub fn map(len: usize) -> *mut u8 {
    // PROT_READ|PROT_WRITE, MAP_PRIVATE|MAP_ANONYMOUS|MAP_NORESERVE
    let p = unsafe { mmap(null_mut(), len, 3, 0x22 | 0x4000, -1, 0) };
    assert!(p as isize != -1);
    p
}

pub fn map_aligned(len: usize, align: usize) -> *mut u8 {
    let p = map(len + align) as usize;
    ((p + align - 1) & !(align - 1)) as *mut u8
}

fn thread_pointer() -> u64 {
    let tp: u64;
    unsafe { std::arch::asm!("mrs {}, tpidr_el0", out(reg) tp) };
    tp
}

impl Env {
    pub fn new(cpu: usize) -> Env {
        let env = Env {
            cpu,
            state: map_aligned(1 << 22, 1 << 16),
            arr: {
                let arr = map_aligned(1 << 22, 1 << 14);
                unsafe {
                    crate::tables::prepare_streams(arr);
                    crate::tables::prepare_sizes(arr);
                }
                arr
            },
            arena: map_aligned(1 << 30, 1 << 24),
            mags: map_aligned(1 << 24, 1 << 20),
            rseq_off: unsafe { __rseq_offset } as u64,
        };
        unsafe { crate::tables::prepare_c(&env) };
        env
    }

    pub fn describe(&self) -> String {
        let (off, size) = unsafe { (__rseq_offset, __rseq_size) };
        let tp = thread_pointer();
        let area = (tp as i64 + off as i64) as *const u32;
        let cpu_id = unsafe { area.add(1).read_volatile() };
        format!("cpu {} rseq offset {off} size {size} cpu_id {cpu_id}", self.cpu)
    }

    pub fn aborts(&self) -> u64 {
        unsafe { (&raw const crate::tables::rf_abort_count).read_volatile() }
    }

    /// Prepares the kind's memory and returns x1, x2, x3 and x4 for the kernel.
    pub fn prepare(&self, kind: Kind) -> (*mut u8, u64, *mut u8, u64) {
        use crate::tables;
        unsafe {
            match kind {
                Kind::Simple => {
                    self.state.write_bytes(0, 1 << 18);
                    (self.state, self.rseq_off, self.arr, 0)
                }
                Kind::Slab => {
                    tables::prepare(self);
                    (tables::slab(self.cpu), self.rseq_off, self.arr, tables::CONST_WORD as u64)
                }
                Kind::List => {
                    tables::prepare_list(self);
                    (tables::slab(self.cpu), self.rseq_off, self.arr, tables::CONST_WORD as u64)
                }
                Kind::Tcm => {
                    tables::prepare_tcm(self);
                    (self.state, self.rseq_off, self.arr, tables::TCM_CLASS as u64)
                }
            }
        }
    }
}
