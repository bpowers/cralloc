//! The offsets of the fields of a thread's restartable-sequence area that the routines address
//! directly: the kernel's `struct rseq` (`include/uapi/linux/rseq.h`). glibc registers an area
//! for every thread and publishes its offset from the thread pointer as `__rseq_offset`.
//!
//! The caches are indexed by the CPU number in `cpu_id`, which every registration has. The
//! concurrency ID (`mm_cid`) is not used: see "What Linux provides" in the design.

/// Offset of `cpu_id`. It is -1 before the thread registers and -2 if its registration failed,
/// so a thread is registered exactly when this field is not negative.
pub const CPU_ID: usize = 4;
/// Offset of `rseq_cs`, the pointer to the descriptor of the sequence in flight.
pub const RSEQ_CS: usize = 8;
