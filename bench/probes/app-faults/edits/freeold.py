import os
import sys
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from sub import sub

# After freecap.py: the free side's purge skips regions released into within about 60 ms.
sub("crates/asmalloc/src/back/purge.rs", "const FREE_SCOPE: Scope = Scope::All;", "const FREE_SCOPE: Scope = Scope::Old;")
