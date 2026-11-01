module

public import Cralloc.Proof.Framework.X86_64.Exec
public import Cralloc.Proof.Framework.X86_64.RegUpd
public import Cralloc.Proof.Framework.X86_64.Access
public import Cralloc.Proof.Framework.Offset

/-!
# Example: thread-local memory through the FS segment

The shape of an rseq critical section's first accesses: `rdx` holds the
offset of a thread-local block from the FS base (as glibc's `__rseq_offset`
does for the thread's rseq area), the code stores a word into it and reads
another word of it back. The block is one more writable region, at
`fsBase + rdx`.
-/

@[expose] public section

namespace VG.Proof.Examples.X86_64.Fs

open VG.X86_64

/-- `mov QWORD PTR fs:[rdx+8], r10; mov rax, QWORD PTR fs:[rdx]` -/
def storeThenLoad : List Instr := [
  .store { base := .rdx, disp := 8, seg := some .fs } .r10,
  .mov .rax (.mem { base := .rdx, seg := some .fs })]

/-- With the 16 bytes at `fsBase + rdx` writable, the code stores `r10` at
offset 8 and loads the unchanged word at offset 0, and changes no other
memory. -/
theorem storeThenLoad_ok (s : State) (hr : ⟨s.fsBase + s.gpr .rdx, 16⟩ ∈ s.wr) :
    WP isa (.block storeThenLoad) s fun s' =>
      s'.gpr .rax = s.mem.readW (s.fsBase + s.gpr .rdx) 64 ∧
      s'.mem.readW (s.fsBase + s.gpr .rdx + 8) 64 = s.gpr .r10 ∧
      Frame [⟨s.fsBase + s.gpr .rdx + 8, 8⟩] s.mem s'.mem := by
  -- The block, and the two words of it the code accesses.
  generalize ht : s.fsBase + s.gpr .rdx = t at hr
  have h8 : (⟨t, 16⟩ : Region).Contains (t + BitVec.ofNat 64 8) 8 :=
    Offset.contains_base t (by decide) (by decide)
  have h0 : (⟨t, 16⟩ : Region).Contains t 8 := by simp [Region.Contains]
  have hst : InRegions s.wr (t + BitVec.ofNat 64 8) 8 := InRegions.of_mem hr h8
  have hld : InRegions (s.rd ++ s.wr) t 8 := (InRegions.of_mem hr h0).append_right
  have hsep : Mem.Sep t (64 / 8) (t + BitVec.ofNat 64 8) (64 / 8) :=
    Offset.sep_base t (by decide) (by decide)
  apply WP.of_runBlock
  -- The load's side condition is on the state after the store, whose
  -- regions `simp` reads as `s`'s, which `hld` gives.
  simp only [storeThenLoad, runBlock_cons, runStep_some, runBlock_nil, exec, readSrc,
    Access.ea_fs_base_disp, BitVec.ofInt_ofNat, ← BitVec.add_assoc, BitVec.add_zero, ht,
    Access.store64_eq hst, Access.load64_eq, hld, Option.map_some,
    Option.some.injEq, exists_eq_left', RegUpd.gpr_setReg_self, RegUpd.mem_setReg]
  refine ⟨?_, ?_, ?_⟩
  · exact Mem.readW_writeW_sep hsep (by decide)
  · exact Mem.readW_writeW_self64 _ _ _
  · exact (Frame.refl _ _).writeW (List.mem_singleton_self _) _ (Region.contains_self _ _)

end VG.Proof.Examples.X86_64.Fs
