import os
import sys
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from sub import sub

# A constant hold in fine steps of about 2 ms, from HOLD_STEPS in the environment (default 32):
# the hold does not adapt.
steps = int(os.environ.get("HOLD_STEPS", "32"))
P = "crates/asmalloc/src/back/policy.rs"
sub(P, "pub const HOLD_STEPS: u32 = 32;", f"pub const HOLD_STEPS: u32 = {steps};")
if "pub const HOLD_ADAPTS: bool = true;" in open(P).read():
    sub(P, "pub const HOLD_ADAPTS: bool = true;", "pub const HOLD_ADAPTS: bool = false;")
