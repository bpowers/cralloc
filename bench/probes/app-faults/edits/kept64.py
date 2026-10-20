import os
import sys
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from sub import sub

sub("crates/asmalloc/src/back/policy.rs", "pub const KEPT_GROWN_HUGE_PAGES: usize = 8;", "pub const KEPT_GROWN_HUGE_PAGES: usize = 32;")
