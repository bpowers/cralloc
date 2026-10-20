import os
import sys
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from sub import sub

# A move always keeps the slot it leaves, resident and counted, as a free does.
sub("crates/asmalloc/src/back/class.rs",
    "                self.keep_grown(class, region::charge(host, *region, *index))\n",
    "                {\n                    self.retain(class, region::charge(host, *region, *index));\n                    true\n                }\n")
