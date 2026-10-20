import os
import sys
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from sub import sub

# The commit-ahead rule as on main: a fixed 64 KiB (or one huge page) ahead.
sub("crates/asmalloc/src/back/policy.rs", """pub const COMMIT_AHEAD_MAX_HUGE_PAGES: usize = 32;""",
    """pub const COMMIT_AHEAD_MAX_HUGE_PAGES: usize = 0;""")
sub("crates/asmalloc/src/back/policy.rs", """    if used < least {
        least""", """    if true || used < least {
        least""")
