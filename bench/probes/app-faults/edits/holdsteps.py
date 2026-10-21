import os
import sys
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from sub import sub

# A hold of HOLD_STEPS fine steps of about 2 ms, from the environment (default 64).
steps = int(os.environ.get("HOLD_STEPS", "64"))
sub("crates/asmalloc/src/back/policy.rs", "pub const HOLD_STEPS: u32 = 64;",
    f"pub const HOLD_STEPS: u32 = {steps};")
