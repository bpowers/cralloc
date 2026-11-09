module

public import Cralloc.TCB.AArch64.Isa

/-! # Pipeline self-test: AArch64 implementation -/

@[expose] public section

namespace VG.Impl.Selftest.AArch64

open VG.AArch64

/-- `x0 := x0 + x1` -/
def add : Prog isa := .block [.add .x .x0 .x0 (.reg .x1)]

/-- `x0 := x3 + x2`, inline: the arguments are not in `x0` and `x1` there. -/
def addInline : Prog isa := .block [.add .x .x0 .x3 (.reg .x2)]

end VG.Impl.Selftest.AArch64
