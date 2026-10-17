//! A stand-in allocator for the inlining study. It is NOT a correct allocator: the slow paths
//! ignore preemption and migration, full magazines are leaked and nothing is ever returned.
//! Only the shape of the fast path matters here: a small `#[inline]` function, in plain Rust
//! or as an `asm!` block with an rseq descriptor.
use std::alloc::{GlobalAlloc, Layout, System};

pub struct Fast;

const SMALL_MAX: usize = 1024;

/// Byte offset of the class's word in a per-thread or per-CPU header array (16-byte classes).
#[inline(always)]
fn class_offset(l: Layout) -> usize {
    ((l.size() + 15) >> 4) << 3
}

unsafe impl GlobalAlloc for Fast {
    #[inline]
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        if l.size() <= SMALL_MAX && l.align() <= 16 {
            let p = imp::pop(class_offset(l));
            if !p.is_null() {
                return p;
            }
            imp::alloc_slow(class_offset(l))
        } else {
            System.alloc(l)
        }
    }

    #[inline]
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        if l.size() <= SMALL_MAX && l.align() <= 16 {
            if !imp::push(class_offset(l), p) {
                imp::dealloc_slow(class_offset(l), p)
            }
        } else {
            System.dealloc(p, l)
        }
    }
}

/// Plain Rust: per-thread intrusive free lists behind a thread-local heap pointer.
#[cfg(not(feature = "asm"))]
mod imp {
    use super::*;
    use std::cell::Cell;
    use std::ptr::null_mut;

    const WORDS: usize = SMALL_MAX / 16 + 1;

    // Only ever read and written through `slot`, by byte offset.
    #[allow(dead_code)]
    struct Heap([Cell<*mut u8>; WORDS]);
    // The shared empty heap is never written: popping from it always misses and pushing to it
    // is refused, so the first operation on a thread takes the slow path.
    struct Empty(Heap);
    unsafe impl Sync for Empty {}
    static EMPTY: Empty = Empty(Heap([const { Cell::new(null_mut()) }; WORDS]));

    thread_local! {
        static HEAP: Cell<*const Heap> = const { Cell::new(&EMPTY.0 as *const Heap) };
    }

    #[inline(always)]
    unsafe fn slot(h: *const Heap, off: usize) -> *const Cell<*mut u8> {
        (h as *const u8).add(off) as *const Cell<*mut u8>
    }

    #[inline(always)]
    pub unsafe fn pop(off: usize) -> *mut u8 {
        let s = slot(HEAP.get(), off);
        let p = (*s).get();
        if !p.is_null() {
            (*s).set((p as *mut *mut u8).read());
        }
        p
    }

    #[inline(always)]
    pub unsafe fn push(off: usize, p: *mut u8) -> bool {
        let h = HEAP.get();
        if std::ptr::eq(h, &EMPTY.0) {
            return false;
        }
        let s = slot(h, off);
        (p as *mut *mut u8).write((*s).get());
        (*s).set(p);
        true
    }

    unsafe fn heap() -> *const Heap {
        let mut h = HEAP.get();
        if std::ptr::eq(h, &EMPTY.0) {
            h = System.alloc_zeroed(Layout::new::<Heap>()) as *const Heap;
            HEAP.set(h);
        }
        h
    }

    #[cold]
    #[inline(never)]
    pub unsafe fn alloc_slow(off: usize) -> *mut u8 {
        let size = off * 2;
        let n = 65536 / size;
        let chunk = System.alloc(Layout::from_size_align_unchecked(n * size, 16));
        for i in 1..n - 1 {
            (chunk.add(i * size) as *mut *mut u8).write(chunk.add((i + 1) * size));
        }
        (chunk.add((n - 1) * size) as *mut *mut u8).write(null_mut());
        (*slot(heap(), off)).set(chunk.add(size));
        chunk
    }

    #[cold]
    #[inline(never)]
    pub unsafe fn dealloc_slow(off: usize, p: *mut u8) {
        let s = slot(heap(), off);
        (p as *mut *mut u8).write((*s).get());
        (*s).set(p);
    }
}

/// An rseq critical section in `asm!`: per-CPU magazines indexed by `mm_cid`, one word of
/// state per class, a single committing store. A magazine is a 512-byte aligned block of 64
/// pointer slots; items live in slots 1..=62 and the state word points at the next free slot.
/// An all-zero state word reads as both empty and full.
#[cfg(feature = "asm")]
mod imp {
    use super::*;
    use std::arch::asm;
    use std::cell::UnsafeCell;

    const CIDS: usize = 64;

    #[repr(C, align(4096))]
    struct Slab(UnsafeCell<[usize; 512 * CIDS]>);
    unsafe impl Sync for Slab {}
    static SLAB: Slab = Slab(UnsafeCell::new([0; 512 * CIDS]));

    extern "C" {
        /// Offset of glibc's registered `struct rseq` from the thread pointer.
        static __rseq_offset: isize;
    }

    // The 32-byte descriptor and the abort landing pad that every critical section needs.
    // Labels: 2 restart, 3 start, 4 end (post-commit), 7 descriptor, 8 abort.
    macro_rules! descriptor {
        () => {
            concat!(
                ".pushsection __rseq_cs, \"aw\"\n",
                ".balign 32\n",
                "7:\n",
                ".long 0, 0\n",
                ".quad 3b, 4b - 3b, 8f\n",
                ".popsection\n",
                ".pushsection .text.unlikely, \"ax\"\n",
                ".long 0x53053053\n",
                "8:\n",
                "jmp 2b\n",
                ".popsection\n",
            )
        };
    }

    /// Returns null when the magazine is empty or absent. A `label` operand cannot be combined
    /// with a real output on stable Rust, so the miss is reported through the result.
    #[inline(always)]
    pub unsafe fn pop(off: usize) -> *mut u8 {
        let item: *mut u8;
        asm!(
            "2:",
            "xor {item:e}, {item:e}",
            "lea {tmp}, [rip + 7f]",
            "mov qword ptr fs:[{rs} + 8], {tmp}",
            "3:",
            "mov {r:e}, dword ptr fs:[{rs} + 24]",
            "shl {r}, 12",
            "add {r}, {slab}",
            "mov {t}, qword ptr [{r} + {off}]",
            "test {t:e}, 0x1f0",
            "jz 4f",
            "mov {item}, qword ptr [{t} - 8]",
            "lea {t}, [{t} - 8]",
            "mov qword ptr [{r} + {off}], {t}",
            "4:",
            descriptor!(),
            rs = in(reg) __rseq_offset,
            slab = in(reg) SLAB.0.get(),
            off = in(reg) off,
            tmp = out(reg) _,
            r = out(reg) _,
            t = out(reg) _,
            item = out(reg) item,
            options(nostack),
        );
        item
    }

    /// Returns false when the magazine is full or absent.
    #[inline(always)]
    pub unsafe fn push(off: usize, p: *mut u8) -> bool {
        asm!(
            "2:",
            "lea {tmp}, [rip + 7f]",
            "mov qword ptr fs:[{rs} + 8], {tmp}",
            "3:",
            "mov {r:e}, dword ptr fs:[{rs} + 24]",
            "shl {r}, 12",
            "add {r}, {slab}",
            "mov {t}, qword ptr [{r} + {off}]",
            "lea {tmp}, [{t} + 8]",
            "test {tmp:e}, 0x1f0",
            "jz {miss}",
            "mov qword ptr [{t}], {p}",
            "mov qword ptr [{r} + {off}], {tmp}",
            "4:",
            descriptor!(),
            rs = in(reg) __rseq_offset,
            slab = in(reg) SLAB.0.get(),
            off = in(reg) off,
            p = in(reg) p,
            tmp = out(reg) _,
            r = out(reg) _,
            t = out(reg) _,
            miss = label { return false },
            options(nostack),
        );
        true
    }

    unsafe fn header(off: usize) -> *mut usize {
        let tp: usize;
        asm!("mov {}, qword ptr fs:[0]", out(reg) tp, options(nostack, readonly, preserves_flags));
        let cid = ((tp as isize + __rseq_offset + 24) as *const u32).read_volatile() as usize;
        (SLAB.0.get() as *mut u8).add(cid * 4096 + off) as *mut usize
    }

    unsafe fn magazine() -> *mut usize {
        System.alloc_zeroed(Layout::from_size_align_unchecked(512, 512)) as *mut usize
    }

    #[cold]
    #[inline(never)]
    pub unsafe fn alloc_slow(off: usize) -> *mut u8 {
        let size = off * 2;
        let h = header(off);
        let mag = if *h == 0 { magazine() } else { (*h & !511) as *mut usize };
        let chunk = System.alloc(Layout::from_size_align_unchecked(62 * size, 16));
        for i in 0..61 {
            mag.add(1 + i).write(chunk.add(i * size) as usize);
        }
        *h = mag.add(62) as usize;
        chunk.add(61 * size)
    }

    #[cold]
    #[inline(never)]
    pub unsafe fn dealloc_slow(off: usize, p: *mut u8) {
        // A full magazine is simply dropped: this is an experiment, not an allocator.
        let mag = magazine();
        mag.add(1).write(p as usize);
        *header(off) = mag.add(2) as usize;
    }
}
