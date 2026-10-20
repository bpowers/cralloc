import os
import sys
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from sub import sub

# After freecap.py: growth over the cap also skips regions released into within about 60 ms.
sub("crates/asmalloc/src/back/purge.rs",
    "                self.purge_step(PURGE_CAP_WORDS - spent, goal - purged, Scope::All);\n",
    "                self.purge_step(PURGE_CAP_WORDS - spent, goal - purged, Scope::Old);\n")
