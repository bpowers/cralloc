import os
import sys
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from sub import sub

# Growth never pays for retained memory.
sub("crates/asmalloc/src/back/purge.rs",
    "        let goal = growth_goal(debt, self.retained(), retained_cap(geometry), step);\n",
    "        let goal = growth_goal(debt, self.retained(), retained_cap(geometry), step) * 0;\n")
