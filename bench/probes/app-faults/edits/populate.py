import os
import sys
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from sub import sub

# Experiment: before a large block that grew is copied into its new slot, the pages the copy
# will write are populated in one call (MADV_POPULATE_WRITE) instead of a fault each.
sub("crates/asmalloc/src/os/mod.rs", """    /// Advises for or against transparent huge pages over a range.""", """    /// Makes a range's pages present and writable, as writes to each would (`MADV_POPULATE_WRITE`).
    ///
    /// # Safety
    ///
    /// The range follows the rules in the module documentation.
    unsafe fn populate(&self, start: NonNull<u8>, len: usize) -> Result<(), OsError> {
        let _ = (start, len);
        Ok(())
    }

    /// Advises for or against transparent huge pages over a range.""")
sub("crates/asmalloc/src/os/linux/kernel.rs", """const MADV_WIPEONFORK: usize = 18;""", """const MADV_WIPEONFORK: usize = 18;
const MADV_POPULATE_WRITE: usize = 23;""")
sub("crates/asmalloc/src/os/linux/kernel.rs", """    unsafe fn purge(&self, start: NonNull<u8>, len: usize) -> Result<(), OsError> {""", """    unsafe fn populate(&self, start: NonNull<u8>, len: usize) -> Result<(), OsError> {
        // SAFETY: KERNEL-03. the caller passes whole pages of a live, writable reservation;
        // populating them for writing changes no contents.
        unsafe { range_call(nr::MADVISE, start, len, MADV_POPULATE_WRITE) }
    }

    unsafe fn purge(&self, start: NonNull<u8>, len: usize) -> Result<(), OsError> {""")
sub("crates/asmalloc/src/space/reservation.rs", """    /// Unmaps the whole reservation.""", """    /// Populates a range of the reservation for writing. Aborts if the range is not one
    /// [`Space::is_callable_range`] accepts.
    pub fn populate(&self, start: NonNull<u8>, len: usize) -> Result<(), OsError> {
        self.check(start, len);
        // SAFETY: RESERVATION-01. as in `commit`; populating changes no contents.
        unsafe { self.os.populate(start, len) }
    }

    /// Unmaps the whole reservation.""")
sub("crates/asmalloc/src/back/class.rs", """        unsafe { self.grown_into(from, to, len) };""", """        unsafe { self.grown_into(from, to, len) };
        if len >= POPULATE_MIN {
            let page = self.space.plan().geometry().page_size();
            let start = to.addr().get() & !(page - 1);
            let end = (to.addr().get() + len).next_multiple_of(page);
            let at = to.map_addr(|_| core::num::NonZeroUsize::new(start).unwrap_or(to.addr()));
            let _ = self.space.populate(at, end - start);
        }""")
sub("crates/asmalloc/src/back/class.rs", """impl<'a, O: Os> Back<'a, O> {""", """/// The least a move copies before it populates the destination in one call.
const POPULATE_MIN: usize = 64 << 10;

impl<'a, O: Os> Back<'a, O> {""")
