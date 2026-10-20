import os
import sys
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from sub import sub

# After freecap.py: every free or launder over the cap pays, not one in a growth step's worth.
sub("crates/asmalloc/src/back/purge.rs",
    "        let step = growth_step(geometry);\n        if FREE_DEBT",
    "        let step = 1;\n        if FREE_DEBT")
