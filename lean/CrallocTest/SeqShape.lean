import Cralloc.TCB.Rust
import Cralloc.TCB.AArch64.Target
import Cralloc.TCB.X86_64.Target
import Cralloc.Artifacts.Slab.X86_64

/-!
# What the emitter refuses in and around a restartable sequence

`Rust.seqShape`, on code that need not be proved to show what it refuses: what would make the
model's run of a sequence differ from the machine's. `CrallocTest/Forgery/` has whole routines
with proofs that it refuses.
-/

namespace VG.Test.SeqShape

/-- Why `seqShape` refuses the code, after the name `f`; `""` if it does not. -/
def why {M : ISA} (P : Printer M) (setup : List M.Instr) (c : Prog M) : String :=
  match Rust.seqShape P "f" setup c with
  | .error e => e
  | .ok _ => ""

section AArch64

open VG.AArch64

def setupA : List Instr := [
  .adrp .x11 .desc,
  .addLo12 .x11 .x11 .desc,
  .ldrWord .x9 .x11 "__rseq_offset",
  .ldr .x .x9 { base := .x9 },
  .mrsTp .x10,
  .add .x .x10 .x10 (.reg .x9)]

def armA : List Instr := [.str .x .x11 { base := .x10, off := .imm 8 }]

def commit : Step Instr Cond := .instr (.str .x .x1 { base := .x0 })

def inBody (i : Instr) : String := why printer setupA (.rseq armA [.instr i, commit])

-- The routines' shape passes, and a setup may form what a body reads.
#guard why printer setupA (.rseq armA [commit]) == ""
#guard why printer (.adrp .x13 (.sym "S") :: setupA) (.rseq armA [commit]) == ""
#guard why printer setupA (.seq (.rseq armA [commit]) (.block [.mov .x .x0 (.reg .x3)])) == ""
#guard why printer setupA (.seq (.rseq armA [commit]) (.jump [.mov .x .x2 (.reg .x4)] "t")) == ""

-- Nothing a linker writes or may move, in a body.
#guard inBody (.adrp .x5 (.sym "S")) ==
  "f's sequence has an instruction the linker writes, which it may also move"
#guard inBody (.adrp .x5 .desc) ==
  "f's sequence has an instruction the linker writes, which it may also move"
#guard inBody (.addLo12 .x5 .x5 (.sym "S")) ==
  "f's sequence has an instruction the linker writes, which it may also move"
#guard inBody (.ldr .x .x5 { base := .x13, off := .lo12 (.sym "S") }) ==
  "f's sequence has a load whose offset the linker writes"
#guard inBody (.str .x .x5 { base := .x13, off := .lo12 (.sym "S") }) ==
  "f's sequence has a store whose offset the linker writes"
-- One instruction to a step, and one store.
#guard inBody (.movGot .x5 "S") == "f's sequence has `movGot`, one step of the model and two instructions"
#guard inBody (.stpz .x5 0) == "f's sequence has `stp`, two stores in one step of the model"
#guard inBody (.ldrWord .x5 .x11 "S") ==
  "f's sequence has a read of the word after its descriptor, which is for its setup"
-- No store whose offset says it is misaligned; a load, and an aligned store, may be in a body.
#guard inBody (.str .x .x5 { base := .x1, off := .imm 4 }) ==
  "f's sequence has a misaligned store, which may not be one access"
#guard inBody (.str .w .x5 { base := .x1, off := .imm 2 }) ==
  "f's sequence has a misaligned store, which may not be one access"
#guard inBody (.str .x .x5 { base := .x1, off := .imm (-8) }) == ""
#guard inBody (.str .b .x5 { base := .x1, off := .imm 3 }) == ""
#guard inBody (.ldr .x .x5 { base := .x1, off := .imm 4 }) == ""

-- The commit is the sequence's last step, and in the arm's place nothing but a store.
#guard why printer setupA (.rseq (.ldr .x .x1 { base := .x0 } :: armA) [commit]) ==
  "f's arm is not exactly one store"
#guard why printer setupA (.rseq [] [commit]) == "f's arm is not exactly one store"
#guard why printer setupA (.rseq [.ldr .x .x1 { base := .x0 }] [commit]) ==
  "f's arm is not exactly one store"

-- An `adrp` ahead of the arm may reach as far as three instructions on.
#guard why printer (setupA ++ [.adrp .x13 (.sym "S")]) (.rseq armA [commit]) ==
  "f's setup ends too near an instruction after which a linker may move one into its sequence"
#guard why printer (setupA ++ [.adrp .x13 (.sym "S"), .mrsTp .x12, .mrsTp .x12]) (.rseq armA [commit]) ==
  "f's setup ends too near an instruction after which a linker may move one into its sequence"
#guard why printer (setupA ++ [.adrp .x13 (.sym "S"), .mrsTp .x12, .mrsTp .x12, .mrsTp .x12])
    (.rseq armA [commit]) == ""
#guard why printer (setupA ++ [.movGot .x13 "S"]) (.rseq armA [commit]) ==
  "f's setup ends too near an instruction after which a linker may move one into its sequence"
-- And what comes before the setup is a caller's, if the function is inlined.
#guard why printer [.mrsTp .x12] (.rseq armA [commit]) ==
  "f's setup has fewer than the 3 instructions a linker's move can span"

-- After the commit, and in an exit's code, nothing runs protected: only registers.
#guard why printer setupA (.seq (.rseq armA [commit]) (.block [.str .x .x1 { base := .x0 }])) ==
  "f's code after its commit accesses memory, outside its restartable sequence"
#guard why printer setupA (.seq (.rseq armA [commit]) (.jump [.ldr .x .x2 { base := .x0 }] "t")) ==
  "f's code after its commit accesses memory, outside its restartable sequence"
#guard why printer setupA (.seq (.rseq armA [commit]) (.jump [.stpz .x0 0] "t")) ==
  "f's code after its commit accesses memory, outside its restartable sequence"
#guard why printer setupA (.seq (.rseq armA [commit]) (.ite .eq (.block []) (.block []))) ==
  "f's code after its commit is not straight-line"
#guard why printer setupA (.rseq armA [.exit .eq [.ldr .x .x2 { base := .x0 }] none, commit]) ==
  "f's exit's code accesses memory, outside its restartable sequence"
#guard why printer setupA (.rseq armA [.exit .eq [.mov .x .x2 (.imm 0)] none, commit]) == ""

-- The descriptor's label names this routine's only ahead of it.
#guard why printer setupA (.seq (.rseq armA [commit]) (.block [.adrp .x6 .desc, .addLo12 .x6 .x6 .desc])) ==
  "f names its descriptor where its label is not that routine's"
#guard why printer setupA (.rseq armA [.exit .hs [.adrp .x6 .desc] none, commit]) ==
  "f names its descriptor where its label is not that routine's"
#guard why printer [] (.block [.adrp .x6 .desc, .addLo12 .x6 .x6 .desc]) ==
  "f names its descriptor where its label is not that routine's"
#guard why printer [.adrp .x6 .desc] (.block []) ==
  "f names its descriptor where its label is not that routine's"

end AArch64

-- `Api.retNonzero` tests the result at its width, so one narrower than a register may not.
#guard match Artifacts.Slab.X86_64.artifacts.find? (·.name == "current_cpu") with
  | some a => match Rust.checkInline { a with retNonzero := true } with
    | .error e => (e.splitOn "as wide as a register").length > 1
    | .ok _ => false
  | none => false

section X86_64

open VG.X86_64

def armX : List Instr := [.leaRseqCs .r11, .store { base := .r10, disp := 8, seg := some .fs } .r11]

def commitX : Step Instr Cond := .instr (.store { base := .rdi } .rax)

-- x86-64's shape passes: a linker moves nothing there, so a body may hold what it writes.
#guard why printer [] (.rseq armX [commitX]) == ""
#guard why printer [] (.rseq armX [.instr (.leaSym .r9 "S"), commitX]) == ""
#guard why printer [] (.seq (.rseq armX [commitX]) (.jump [.mov .rdx (.reg .rax)] "t")) == ""

-- After the commit, and in an exit's code, the same as on AArch64.
#guard why printer [] (.seq (.rseq armX [commitX]) (.jump [.mov .rdx (.mem { base := .rax })] "t")) ==
  "f's code after its commit accesses memory, outside its restartable sequence"
#guard why printer [] (.seq (.rseq armX [commitX]) (.block [.store { base := .rax } .rdx])) ==
  "f's code after its commit accesses memory, outside its restartable sequence"
#guard why printer [] (.rseq armX [.exit .e [.mov .rdx (.mem { base := .rax })] none, commitX]) ==
  "f's exit's code accesses memory, outside its restartable sequence"
#guard why printer [] (.seq (.rseq armX [commitX]) (.block [.leaRseqCs .rax])) ==
  "f names its descriptor where its label is not that routine's"
#guard why printer [] (.rseq armX [.instr (.store { base := .rdi, disp := 4 } .rax), commitX]) ==
  "f's sequence has a misaligned store, which may not be one access"

-- A setup counts against the limit of micro-ops.
#guard match Rust.uopCheck { printer with uopLimit := some 4 } false "f" (.rseq armX [commitX])
    [.mov .rax (.reg .rdi), .mov .rax (.reg .rdi)] with
  | .error e => e == "f's hot path has 6 fused micro-ops, more than 4"
  | .ok _ => false

end X86_64

end VG.Test.SeqShape
