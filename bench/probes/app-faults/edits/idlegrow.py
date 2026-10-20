import os
import sys
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from sub import sub

# Growth pays only from memory no fill reached in the last two intervals.
sub("crates/asmalloc/src/back/purge.rs",
    "                self.purge_step(PURGE_CAP_WORDS - spent, goal - purged, Scope::All);\n",
    "                self.purge_step(PURGE_CAP_WORDS - spent, goal - purged, Scope::Idle);\n")
