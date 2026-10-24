#!/usr/bin/env python3
"""The prototype for docs/research/meili-peak.md, behind a run-time switch: a zeroed large
allocation whose slot is not known to be zero clears its whole pages by returning them to the
OS instead of writing zeros over them, so that pages the program never writes are never
resident. Never product code: apply it to a scratch copy (build.sh), after peak_edit.py or
alone.

  proto_edit.py TREE

`ASMALLOC_ZPURGE=<bytes>`, read once: a zeroed large allocation with at least that many bytes
to clear returns the whole pages of them (`madvise(MADV_DONTNEED)`) and writes zeros only over
the part pages at either end. Unset or 0: main's behaviour, a `memset` of every byte.
`ASMALLOC_ZRES=<k>`: return them only if `mincore` finds fewer than k/64 of the whole pages
resident, else clear them as main does; unset or 0, return them whatever is resident. The
returned pages stay counted as the block's (their clean bits stay clear), so the retained
count over-counts them when the block is freed, as it does any page a block never wrote.
"""
import sys
from pathlib import Path

TREE = Path(sys.argv[1])
SRC = TREE / "crates/asmalloc/src"
DIAG = (SRC / "diag.rs").exists()


def sub(path, old, new):
    p = SRC / path
    s = p.read_text()
    if s.count(old) != 1:
        raise SystemExit(f"{path}: {s.count(old)} matches for {old[:80]!r}")
    p.write_text(s.replace(old, new))


PROTO = r'''//! Scratch prototype (bench/probes/meili-peak/proto_edit.py). Not product code.
#![allow(missing_docs, dead_code)]
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering::Relaxed};

unsafe extern "C" {
    fn getenv(name: *const core::ffi::c_char) -> *const core::ffi::c_char;
}

static ZPURGE: AtomicUsize = AtomicUsize::new(usize::MAX);
static ZRES: AtomicUsize = AtomicUsize::new(usize::MAX);

/// `ASMALLOC_ZRES`, read once: return a block's pages only if fewer than this many 64ths of
/// them are resident; zero (unset) returns them whatever is resident.
pub fn zres() -> usize {
    number(&ZRES, c"ASMALLOC_ZRES")
}

/// Calls and bytes returned instead of cleared, and calls that fell back to clearing.
#[unsafe(no_mangle)]
pub static ASMALLOC_PROTO: [AtomicU64; 4] = [const { AtomicU64::new(0) }; 4];

/// `ASMALLOC_ZPURGE`, read once; zero when unset or not a number.
pub fn zpurge() -> usize {
    number(&ZPURGE, c"ASMALLOC_ZPURGE")
}

fn number(cell: &AtomicUsize, name: &core::ffi::CStr) -> usize {
    let seen = cell.load(Relaxed);
    if seen != usize::MAX {
        return seen;
    }
    // SAFETY: getenv returns null or a NUL-terminated string.
    let p = unsafe { getenv(name.as_ptr()) };
    let mut v = 0usize;
    if !p.is_null() {
        let mut i = 0;
        loop {
            // SAFETY: within the NUL-terminated string, which ends at a byte that is no digit.
            let c = unsafe { *p.add(i) } as u8;
            if !c.is_ascii_digit() {
                break;
            }
            v = v.saturating_mul(10).saturating_add((c - b'0') as usize);
            i += 1;
        }
    }
    cell.store(v, Relaxed);
    v
}
'''
(SRC / "proto.rs").write_text(PROTO)
sub("lib.rs", "mod heap;\n", "mod heap;\npub mod proto;\n")

count = ("                            crate::diag::add(crate::diag::PROTO_0, 1);\n"
         "                            crate::diag::add(crate::diag::PROTO_0 + 1, end - first);\n") if DIAG else ""
sub("heap/large.rs", """                // SAFETY: HEAP-03.
                unsafe { block.ptr.as_ptr().write_bytes(0, clear) };
                Some(Grant {""", f"""                let page = back.space().plan().geometry().page_size();
                let threshold = crate::proto::zpurge();
                let start = block.ptr.addr().get();
                let (first, end) = (start.next_multiple_of(page), (start + clear) & !(page - 1));
                let mut cleared = false;
                let at = block.ptr.map_addr(|a| a.saturating_add(first - start));
                // With ASMALLOC_ZRES=k, only a block with fewer than k/64 of its whole pages
                // resident is returned; one more resident than that is cleared as before.
                let mostly_gone = || {{
                    let k = crate::proto::zres();
                    if k == 0 {{
                        return true;
                    }}
                    let pages = (end - first) / page;
                    // SAFETY: scratch prototype; mincore reads no memory, and the block is the
                    // caller's.
                    let resident = unsafe {{ back.space().resident(at, end - first) }}.unwrap_or(pages);
                    resident * 64 < pages * k
                }};
                if threshold != 0 && clear >= threshold && end > first && mostly_gone() {{
                    // SAFETY: scratch prototype. The caller has just been given the block and
                    // nothing reads or writes it yet; the range is whole pages inside it.
                    if unsafe {{ back.space().purge(at, end - first) }}.is_ok() {{
                        // SAFETY: HEAP-03, for the part pages at either end.
                        unsafe {{
                            block.ptr.as_ptr().write_bytes(0, first - start);
                            block.ptr.as_ptr().add(end - start).write_bytes(0, start + clear - end);
                        }}
                        crate::proto::ASMALLOC_PROTO[0].fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                        crate::proto::ASMALLOC_PROTO[1].fetch_add((end - first) as u64, core::sync::atomic::Ordering::Relaxed);
{count}                        cleared = true;
                    }} else {{
                        crate::proto::ASMALLOC_PROTO[2].fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                    }}
                }}
                if !cleared {{
                    // SAFETY: HEAP-03.
                    unsafe {{ block.ptr.as_ptr().write_bytes(0, clear) }};
                }}
                Some(Grant {{""")
print("prototype edits applied to", TREE)
