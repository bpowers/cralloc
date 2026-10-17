import Asmalloc.TCB.X86_64.Target
import Asmalloc.Proof.Framework.Semantics
import Asmalloc.Proof.Framework.X86_64.RegUpd

/-!
# A vacuous fast path cannot be verified

A review built an accepted `Artifact` for a pop whose fast path stores into
read-only memory and loads from memory it was never granted: its
precondition forced the slab word to zero, so only the slow branch ever ran,
and one state satisfying the precondition was all `Verified` asked. Now each
branch must be taken by some state satisfying the precondition
(`Code.outcomes`, `Visits`), and this file proves that this pop cannot be
verified, whatever its proof.
-/

open VG VG.X86_64

namespace VG.Test.Vacuous

def popSig : Sig := { params := [], ret := some .u64 }

def popContracts : Contracts := fun A stack =>
  popSig.contract A (post := fun _m _m' _r => True) (stack := stack)

/-- The slab array as a table of constants holding one zero word. -/
def slabs : List (String × List (BitVec 64)) := [("SLABS", [0])]

/-- `lea r9,[rip+SLABS]; mov rcx,[r9]; test ecx,0x1f0; je slow;
mov rax,[rcx-8]; lea rcx,[rcx-8]; mov [r9],rcx` -/
def pop : Prog isa :=
  .seq (.block [.leaSym .r9 "SLABS", .mov .rcx (.mem { base := .r9 }), .alu32 .test .rcx (.imm 0x1f0)])
    (Code.iteCold .e (.block [.movImm64 .rax 0])
      (.block [.mov .rax (.mem { base := .rcx, disp := -8 }), .lea .rcx { base := .rcx, disp := -8 },
               .store { base := .r9 } .rcx]))

/-- The fast path, the `ite`'s `false` way, is one of the outcomes. -/
theorem fast_is_outcome : ((0 : Nat), false) ∈ pop.outcomes := by decide

/-- No state satisfying the precondition takes the fast path: the precondition
says the slab word is zero, so `test` sets ZF and the branch goes to the slow
path. -/
theorem no_fast_witness (s : State) (hs : (popContracts (abi.withConsts slabs) 0).pre s) :
    ¬ Visits isa pop s 0 false := by
  intro hv
  simp only [popContracts, Sig.contract, Abi.withConsts, abi, slabs, popSig, Sig.words, argRegs,
    List.length, List.map, List.flatMap] at hs
  obtain ⟨⟨-, -, hheld, -⟩, -⟩ := hs
  have hz : s.mem.readW (s.syms "SLABS") 64 = 0 := by
    simpa [Abi.constsHeld] using hheld _ (List.mem_singleton_self _) 0 (by decide)
  generalize hi : (0 : Nat) = i at hv
  generalize hd : false = d at hv
  cases hv with
  | seqL h => cases h
  | seqR he hite =>
    rw [Exec.block_iff] at he
    -- The block loads the slab word (zero) and tests it: ZF is set.
    have hzf : ∀ s₁, execBlock isa [.leaSym .r9 "SLABS", .mov .rcx (.mem { base := .r9 }),
        .alu32 .test .rcx (.imm 0x1f0)] s = some s₁ → s₁.zf = some true := by
      intro s₁ h
      by_cases hin : InRegions (s.rd ++ s.wr) (s.syms "SLABS") 8
      · simp [execBlock, isa, exec, readSrc, State.ea, State.offset, State.load64, readSrc32,
          execAlu32, State.setReg, hin, hz] at h
        subst h
        rfl
      · simp [execBlock, isa, exec, readSrc, State.ea, State.offset, State.load64, State.setReg,
          hin] at h
    cases hite with
    | ite hc =>
      subst hd
      simp [eval, hzf _ he] at hc
    | iteT _ _ => simp [Code.sites] at hi; omega
    | iteF _ _ => simp [Code.sites] at hi; omega

/-- So this pop is not `Verified` under its contract, whatever its proof. -/
theorem not_verified : ¬ Verified target pop (popContracts (abi.withConsts slabs) 0) := by
  intro h
  obtain ⟨s, hs, hv⟩ := h.2.2.2 _ fast_is_outcome
  exact no_fast_witness s hs hv

end VG.Test.Vacuous
