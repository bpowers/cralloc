import os
import sys
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from sub import sub

# After largeidle.py: hold large free slots for about 250 ms (2^30 ticks) instead of 60.
sub("crates/asmalloc/src/back/purge.rs", "const OLD_TICKS: u64 = 1 << 28;", "const OLD_TICKS: u64 = 1 << 30;")
