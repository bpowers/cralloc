import os
import sys
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from sub import sub

# Experiment: frees do not stamp their regions (`back::hold`), to see what the stamp costs.
sub("crates/asmalloc/src/back/hold.rs",
    "    pub(super) fn hold_stamp(&self, host: &ClassHost<'_, O>, region: RegionId) {\n",
    "    pub(super) fn hold_stamp(&self, host: &ClassHost<'_, O>, region: RegionId) {\n"
    "        if true {\n            return;\n        }\n")
