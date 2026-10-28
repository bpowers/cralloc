#!/usr/bin/env python3
"""Makes the memstat sampler write to ASMALLOC_MEMSTAT.<pid>, so a driver and the server it
runs as a child of the same binary do not share one file.  ms-pid.py MEMSTAT_RS"""
import sys
from pathlib import Path

p = Path(sys.argv[1])
s = p.read_text()
old = "    let fd = unsafe { libc::open(path, libc::O_WRONLY | libc::O_CREAT | libc::O_TRUNC | libc::O_CLOEXEC, 0o644) };\n"
new = """    let mut named = [0u8; 512];
    // SAFETY: `path` is a NUL-terminated string.
    let base = unsafe { std::ffi::CStr::from_ptr(path) }.to_bytes();
    let suffix = format!(".{}\\0", std::process::id());
    if base.len() + suffix.len() > named.len() {
        return;
    }
    named[..base.len()].copy_from_slice(base);
    named[base.len()..base.len() + suffix.len()].copy_from_slice(suffix.as_bytes());
    // SAFETY: `named` is NUL-terminated.
    let fd = unsafe { libc::open(named.as_ptr().cast(), libc::O_WRONLY | libc::O_CREAT | libc::O_TRUNC | libc::O_CLOEXEC, 0o644) };
"""
if s.count(old) != 1:
    raise SystemExit("open call not found once")
p.write_text(s.replace(old, new))
