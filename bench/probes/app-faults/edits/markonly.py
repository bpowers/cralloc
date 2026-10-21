import os
import sys
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from sub import sub

# Experiment: a free writes a constant to its region's stamp word instead of the fine count, to
# see what reading the fine count costs a free (`back::hold`). Not a working hold.
sub("crates/asmalloc/src/back/hold.rs",
    "        let stamp = stamp_of(self.state.hold.fine.count());\n",
    "        let stamp = stamp_of(7);\n")
