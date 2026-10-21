import os
import sys
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from sub import sub

# The branch with the hold on free large slots switched off: frees stamp nothing, heartbeats
# and `maintain` look at nothing.
H = "crates/asmalloc/src/back/hold.rs"
for head, value in [
    ("    pub(super) fn hold_stamp(&self, host: &ClassHost<'_, O>, region: RegionId) {\n", ""),
    ("    pub(super) fn hold_beat(&self, time: u64) {\n", ""),
    ("    pub fn hold_maintain_step(&self, now: &dyn Fn() -> u64) -> bool {\n", " false"),
]:
    sub(H, head, head + f"        if true {{\n            return{value};\n        }}\n")
