import os
import sys
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from sub import sub

# Diagnostic: resident bytes each purge walk returned, by caller and log2 of the class size
# (kind 100 + log2), printed as "walkN".
sub("crates/asmalloc/src/back/purge.rs",
    "        self.unretain(class, purged);\n        (purged, read, done)\n",
    "        self.unretain(class, purged);\n"
    "        if purged != 0 {\n"
    "            crate::os::linux::diag::note(100 + (63 - class.size().leading_zeros()) as u64, core::panic::Location::caller(), purged, 0);\n"
    "        }\n"
    "        (purged, read, done)\n")
sub("crates/asmalloc/src/os/linux/diag.rs",
    '                "DIAG site {} {}:{} calls {} bytes {} ticks {}",\n'
    '                names.get(kind).copied().unwrap_or("?"),\n',
    '                "DIAG site {}{} {}:{} calls {} bytes {} ticks {}",\n'
    '                names.get(kind).copied().unwrap_or("walk"),\n'
    '                if kind >= 100 { kind - 100 } else { 0 },\n')
