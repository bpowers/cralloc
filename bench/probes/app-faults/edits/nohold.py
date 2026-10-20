import os
import sys
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from sub import sub

# The branch without the hold on free large slots: the base it was built on, as a variant.
sub("crates/asmalloc/src/back/hold.rs", """    pub(super) fn hold_step(&self, time: Option<u64>, now: &dyn Fn() -> u64) {
""", """    pub(super) fn hold_step(&self, time: Option<u64>, now: &dyn Fn() -> u64) {
        if true {
            let _ = (time, now);
            return;
        }
""")
sub("crates/asmalloc/src/back/hold.rs", """    pub(super) fn hold_released(&self, class: Class, region: RegionId) {
""", """    pub(super) fn hold_released(&self, class: Class, region: RegionId) {
        if true {
            let _ = (class, region);
            return;
        }
""")
