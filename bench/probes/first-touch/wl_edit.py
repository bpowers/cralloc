#!/usr/bin/env python3
"""The trace of the allocator's address-space calls for docs/research/first-touch.md, part 2,
for measurement only. Applied to a scratch copy of the repository (build.sh), it puts wl.rs into
the allocator as `crate::wl`, times every `mprotect`, `madvise`, `munmap`, `mmap` and `mincore`
the allocator makes, and tags each with what its caller was doing. The allocator's own tests
are not built with this edit.

  wl_edit.py TREE
"""
import shutil
import sys
from pathlib import Path

TREE = Path(sys.argv[1])
HERE = Path(__file__).resolve().parent
SRC = TREE / "crates/asmalloc/src"


def sub(path, old, new, count=1):
    p = SRC / path
    s = p.read_text()
    if s.count(old) != count:
        raise SystemExit(f"{path}: {s.count(old)} matches (want {count}) for {old[:90]!r}")
    p.write_text(s.replace(old, new))


shutil.copy(HERE / "wl.rs", SRC / "wl.rs")
sub("lib.rs", "mod heap;\n", "mod heap;\npub mod wl;\n")

K = "os/linux/kernel.rs"
sub(K, """    result(unsafe {
        syscall(
            number,
            [start.as_ptr(), int(len), int(arg), int(0), int(0), int(0)],
        )
    })
    .map(|_| ())
}""", """    let u0 = crate::wl::usage();
    let t0 = crate::wl::now();
    let r = result(unsafe {
        syscall(
            number,
            [start.as_ptr(), int(len), int(arg), int(0), int(0), int(0)],
        )
    })
    .map(|_| ());
    let t1 = crate::wl::now();
    let op = match (number, arg) {
        (nr::MPROTECT, _) => crate::wl::OP_MPROTECT,
        (nr::MUNMAP, _) => crate::wl::OP_MUNMAP,
        (_, MADV_DONTNEED) => crate::wl::OP_DONTNEED,
        (_, MADV_HUGEPAGE) => crate::wl::OP_HUGEPAGE,
        (_, MADV_NOHUGEPAGE) => crate::wl::OP_NOHUGEPAGE,
        _ => crate::wl::OP_MADV_OTHER,
    };
    crate::wl::record(op, start.addr().get(), len, t0, t1, u0);
    r
}""")
sub(K, """            result(unsafe {
                syscall(
                    nr::MINCORE,""", """            let u0 = crate::wl::usage();
            let t0 = crate::wl::now();
            let r = result(unsafe {
                syscall(
                    nr::MINCORE,""")
sub(K, """                        int(0),
                    ],
                )
            })?;
            count += flags""", """                        int(0),
                    ],
                )
            });
            crate::wl::record(crate::wl::OP_MINCORE, at.addr().get(), pages * page, t0, crate::wl::now(), u0);
            r?;
            count += flags""")
sub(K, """        let mapped = result(unsafe { syscall(nr::MMAP, args) })?;""",
    """        let u0 = crate::wl::usage();
        let t0 = crate::wl::now();
        let mapped = result(unsafe { syscall(nr::MMAP, args) });
        crate::wl::record(crate::wl::OP_MMAP, 0, span, t0, crate::wl::now(), u0);
        let mapped = mapped?;""")

# What the caller was doing.
R = "back/region.rs"
sub(R, """fn commit_range<H: Host>(host: &H, region: RegionId, now: Grow, target: u32) -> Option<u32> {
""", """fn commit_range<H: Host>(host: &H, region: RegionId, now: Grow, target: u32) -> Option<u32> {
    let _wl = crate::wl::Ctx::enter(crate::wl::C_COMMIT);
""")
sub(R, """fn switch_to_huge<H: Host>(host: &H, region: RegionId, need: u32) -> Option<u32> {
""", """fn switch_to_huge<H: Host>(host: &H, region: RegionId, need: u32) -> Option<u32> {
    let _wl = crate::wl::Ctx::enter(crate::wl::C_TO_HUGE);
""")
P = "back/purge.rs"
sub(P, """    fn pay_growth(&self, step: usize) -> usize {
""", """    fn pay_growth(&self, step: usize) -> usize {
        let _wl = crate::wl::Ctx::enter(crate::wl::C_GROWTH);
""")
sub(P, """        returned: &mut usize,
    ) -> Option<u64> {
        let state = self.state;""", """        returned: &mut usize,
    ) -> Option<u64> {
        let _wl = crate::wl::Ctx::enter(crate::wl::C_PASS);
        let state = self.state;""")
sub(P, """    pub fn trim(&self) -> usize {
""", """    pub fn trim(&self) -> usize {
        let _wl = crate::wl::Ctx::enter(crate::wl::C_TRIM);
""")
H = "back/hold.rs"
sub(H, """        reach: Reach,
    ) -> (usize, usize, bool) {
""", """        reach: Reach,
    ) -> (usize, usize, bool) {
        let _wl = crate::wl::Ctx::enter(crate::wl::C_HOLD);
""")
C = "back/class.rs"
sub(C, """        mut observe: impl FnMut(),
    ) {
        let piece = move_piece(""", """        mut observe: impl FnMut(),
    ) {
        let _wl = crate::wl::Ctx::enter(crate::wl::C_MOVE);
        let piece = move_piece(""")
sub(C, """    pub unsafe fn resize_in_place(&self, ptr: NonNull<u8>, size: usize) {
""", """    pub unsafe fn resize_in_place(&self, ptr: NonNull<u8>, size: usize) {
        let _wl = crate::wl::Ctx::enter(crate::wl::C_RESIZE);
""")
sub(C, """    ) -> (usize, bool) {
        let resident = |charge: usize| (charge, charge != 0);""", """    ) -> (usize, bool) {
        let _wl = crate::wl::Ctx::enter(crate::wl::C_FREE);
        let resident = |charge: usize| (charge, charge != 0);""")
sub(C, """    fn alloc_at(&self, class: Class, size: usize, stagger: bool) -> Option<Block> {
""", """    fn alloc_at(&self, class: Class, size: usize, stagger: bool) -> Option<Block> {
        let _wl = crate::wl::Ctx::enter(crate::wl::C_ALLOC);
""")
print("address-space trace applied to", TREE)
