import os
import sys
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from sub import sub

# Diagnostic, after `credit2.py`: a claim of a large slot whose pages were returned is not
# charged to the growth debt (as on main). Carves, and with `covergrow.py` growth in place, still
# are. Tells what charging refaults costs a program that takes its blocks back within the
# ceiling: each charged refault evicts other free memory, which that program then faults back.
sub("crates/asmalloc/src/back/class.rs", """            self.grow(settled.refaulted);
""", "")
