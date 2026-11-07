#!/usr/bin/env python3
"""Run a command with transparent huge pages disabled for the process (PR_SET_THP_DISABLE).

usage: thp-off.py COMMAND [ARGS...]   (run it as `python3 -I thp-off.py ...` inside a cap)

The flag is inherited across fork and kept across execve, and needs no privilege. It disables
THP even in ranges advised with MADV_HUGEPAGE, so every allocator is treated alike: it
separates what huge pages buy an allocator from everything else
(docs/research/program-effects.md, "TLB and huge pages").
"""
import ctypes
import os
import sys

PR_SET_THP_DISABLE = 41
PR_GET_THP_DISABLE = 42
libc = ctypes.CDLL(None, use_errno=True)
if libc.prctl(PR_SET_THP_DISABLE, 1, 0, 0, 0) != 0:
    raise OSError(ctypes.get_errno(), "prctl(PR_SET_THP_DISABLE)")
if libc.prctl(PR_GET_THP_DISABLE, 0, 0, 0, 0) != 1:
    sys.exit("thp-off: the flag did not stick")
os.execvp(sys.argv[1], sys.argv[1:])
