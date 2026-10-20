import os
import sys
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from sub import sub

# Every MADV_DONTNEED fails without a call: no rule returns memory.
sub("crates/asmalloc/src/os/linux/kernel.rs", """    unsafe fn purge(&self, start: NonNull<u8>, len: usize) -> Result<(), OsError> {
""", """    unsafe fn purge(&self, start: NonNull<u8>, len: usize) -> Result<(), OsError> {
        if true {
            return Err(OsError::from_errno(11));
        }
""")
