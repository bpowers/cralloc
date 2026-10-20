import os
import sys
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from sub import sub

# A move keeps the slot it leaves, resident and counted, whenever that slot alone is within
# the kept-grown budget, whatever else is retained.
sub("crates/asmalloc/src/back/class.rs",
    "                self.keep_grown(class, region::charge(host, *region, *index))\n",
    "                {\n"
    "                    let charge = region::charge(host, *region, *index);\n"
    "                    if charge <= kept_grown(self.space.plan().geometry()) {\n"
    "                        self.retain(class, charge);\n"
    "                        true\n"
    "                    } else {\n"
    "                        false\n"
    "                    }\n"
    "                }\n")
