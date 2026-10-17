//! Prints what the kernel offers for rseq and what glibc registered.
//!
//! A registration whose size is at least `AT_RSEQ_FEATURE_SIZE` selects the kernel's newer
//! rseq mode (ID fields written only on change and read-only to user space). Comparing the
//! auxv value with glibc's `__rseq_size` shows which mode a process riding glibc's
//! registration is in. `__rseq_offset` is the area's offset from the thread pointer; it
//! depends on the binary's static TLS layout, so it must be read at run time.
extern "C" {
    fn getauxval(t: u64) -> u64;
    static __rseq_size: u32;
    static __rseq_flags: u32;
    static __rseq_offset: isize;
}

const AT_RSEQ_FEATURE_SIZE: u64 = 27;
const AT_RSEQ_ALIGN: u64 = 28;

fn main() {
    unsafe {
        println!(
            "AT_RSEQ_FEATURE_SIZE={} AT_RSEQ_ALIGN={} __rseq_size={} __rseq_flags={} __rseq_offset={}",
            getauxval(AT_RSEQ_FEATURE_SIZE),
            getauxval(AT_RSEQ_ALIGN),
            __rseq_size,
            __rseq_flags,
            __rseq_offset
        );
    }
}
