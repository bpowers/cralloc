module

public import Asmalloc.TCB.X86_64.Isa

/-! # Pipeline self-test: x86-64 implementation -/

@[expose] public section

namespace VG.Impl.Selftest.X86_64

open VG.X86_64

/-- `rax := rdi + rsi` -/
def add : Prog isa := .block [
  .mov .rax (.reg .rdi),
  .alu .add .rax (.reg .rsi)
]

end VG.Impl.Selftest.X86_64
