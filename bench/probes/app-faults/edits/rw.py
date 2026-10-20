import os
import sys
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from sub import sub

# The reservation is mapped readable and writable (MAP_NORESERVE, so no commit charge under
# heuristic overcommit), and a commit is no call at all.
sub("crates/asmalloc/src/os/linux/kernel.rs", """            int(span),
            int(PROT_NONE),
            int(flags),""", """            int(span),
            int(PROT_READ | PROT_WRITE),
            int(flags),""")
sub("crates/asmalloc/src/os/linux/kernel.rs", """    unsafe fn commit(&self, start: NonNull<u8>, len: usize) -> Result<(), OsError> {
""", """    unsafe fn commit(&self, start: NonNull<u8>, len: usize) -> Result<(), OsError> {
        if true {
            let _ = (start, len);
            return Ok(());
        }
""")
