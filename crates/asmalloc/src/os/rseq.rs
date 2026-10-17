//! Facts about a thread's restartable-sequence registration.
//!
//! glibc registers an rseq area for every thread and publishes where it is: `__rseq_offset`, the
//! area's offset from the thread pointer (`%fs` base on x86-64), and `__rseq_size`, the number of
//! bytes of the area the kernel maintains, or zero when there is no registration. The field
//! offsets below are the kernel's `struct rseq` (`include/uapi/linux/rseq.h`), which the routines
//! address directly.

/// Offset of `cpu_id_start` in the rseq area.
pub const CPU_ID_START: usize = 0;
/// Offset of `cpu_id`. It is -1 before the thread registers and -2 if its registration failed,
/// so a thread is registered exactly when this field is not negative.
pub const CPU_ID: usize = 4;
/// Offset of `rseq_cs`, the pointer to the descriptor of the sequence in flight.
pub const RSEQ_CS: usize = 8;
/// Offset of `flags`.
pub const FLAGS: usize = 16;
/// Offset of `node_id`.
pub const NODE_ID: usize = 20;
/// Offset of `mm_cid`, the concurrency ID.
pub const MM_CID: usize = 24;

/// The 32-bit signature glibc registers on x86-64 (`RSEQ_SIG`). The kernel requires the four
/// bytes before every abort handler to equal it, so the routines must emit the same value.
pub const SIGNATURE: u32 = 0x5305_3053;

/// Where a thread's rseq area is and what it holds.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct RseqArea {
    offset: isize,
    size: u32,
}

impl RseqArea {
    /// The facts for glibc's `__rseq_offset` and `__rseq_size`, or `None` if `size` is zero,
    /// which means the thread has no registration.
    pub const fn new(offset: isize, size: u32) -> Option<RseqArea> {
        if size == 0 {
            None
        } else {
            Some(RseqArea { offset, size })
        }
    }

    /// The area's offset from the thread pointer.
    pub const fn offset(self) -> isize {
        self.offset
    }

    /// The number of bytes of the area the kernel maintains.
    pub const fn size(self) -> u32 {
        self.size
    }

    /// Whether the kernel maintains `mm_cid`, which the fast paths need. A false yes would let
    /// two threads share a slab, so [`super::linux::LinuxOs`] caps glibc's size by the kernel's
    /// own feature size before reporting it.
    pub const fn has_cid(self) -> bool {
        self.size as usize >= MM_CID + 4
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cid_needs_the_first_twenty_eight_bytes() {
        assert_eq!(RseqArea::new(-192, 0), None);
        assert!(!RseqArea::new(-192, 20).unwrap().has_cid());
        assert!(RseqArea::new(-192, 28).unwrap().has_cid());
        assert!(RseqArea::new(-192, 33).unwrap().has_cid());
        assert_eq!(RseqArea::new(-192, 33).unwrap().offset(), -192);
    }
}
