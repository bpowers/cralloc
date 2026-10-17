module

public import Asmalloc.TCB.X86_64.Isa
public import Asmalloc.Proof.Framework.Mem

/-!
# x86-64: memory accesses, for symbolic execution

`State.load64` and its kin are an `if` on whether the access is permitted
(`InRegions`), which `simp` cannot decide from a hypothesis that the access
lies in a region: these lemmas take that fact and give the access's result.
The `ea_*` lemmas give the address of the common forms of memory operand,
with and without an FS segment override, so that a proof rewrites with them
rather than unfolding `State.ea` and `State.offset`.

Use them with `simp only [runBlock_cons, runStep_some, exec, readSrc, …,
Access.load64_eq h, Access.store64_eq h, Access.ea_base, …]`.
-/

@[expose] public section

namespace VG

theorem InRegions.of_mem {rs : List Region} {r : Region} {a : Addr} {n : Nat} (hr : r ∈ rs)
    (hc : r.Contains a n) : InRegions rs a n := ⟨r, hr, hc⟩

theorem InRegions.append_left {rs ws : List Region} {a : Addr} {n : Nat} (h : InRegions rs a n) :
    InRegions (rs ++ ws) a n :=
  let ⟨r, hr, hc⟩ := h; ⟨r, List.mem_append_left _ hr, hc⟩

theorem InRegions.append_right {rs ws : List Region} {a : Addr} {n : Nat} (h : InRegions ws a n) :
    InRegions (rs ++ ws) a n :=
  let ⟨r, hr, hc⟩ := h; ⟨r, List.mem_append_right _ hr, hc⟩

end VG

namespace VG.X86_64.Access

variable {s : State} {a : Addr}

/-! ## Loads and stores that are permitted -/

theorem load64_eq (h : InRegions (s.rd ++ s.wr) a 8) : s.load64 a = some (s.mem.readW a 64) := by
  simp [State.load64, h]

theorem load32_eq (h : InRegions (s.rd ++ s.wr) a 4) : s.load32 a = some (s.mem.readW a 32) := by
  simp [State.load32, h]

theorem load8_eq (h : InRegions (s.rd ++ s.wr) a 1) : s.load8 a = some (s.mem a) := by
  simp [State.load8, h]

theorem store64_eq {v : BitVec 64} (h : InRegions s.wr a 8) :
    s.store64 a v = some { s with mem := s.mem.writeW a v } := by
  simp [State.store64, h]

theorem store32_eq {v : BitVec 32} (h : InRegions s.wr a 4) :
    s.store32 a v = some { s with mem := s.mem.writeW a v } := by
  simp [State.store32, h]

theorem store8_eq {v : Byte} (h : InRegions s.wr a 1) :
    s.store8 a v = some { s with mem := s.mem.writeW a v } := by
  simp [State.store8, h]

/-- A writable access is readable. -/
theorem rd_wr_of_wr {n : Nat} (h : InRegions s.wr a n) : InRegions (s.rd ++ s.wr) a n :=
  InRegions.append_right h

/-! ## Addresses of memory operands -/

theorem ea_of_seg_none {m : MemOp} (h : m.seg = none) : s.ea m = s.offset m := by
  unfold State.ea; rw [h]

theorem ea_of_seg_fs {m : MemOp} (h : m.seg = some .fs) : s.ea m = s.fsBase + s.offset m := by
  unfold State.ea; rw [h]

/-- `[base]` -/
theorem ea_base (b : Reg) : s.ea { base := b } = s.gpr b := by
  simp [State.ea, State.offset]

/-- `[base + disp]` -/
theorem ea_base_disp (b : Reg) (d : Int) : s.ea { base := b, disp := d } = s.gpr b + BitVec.ofInt 64 d :=
  rfl

/-- `[base + index * scale + disp]` -/
theorem ea_index (b i : Reg) (k : Nat) (d : Int) :
    s.ea { base := b, index := some i, scale := k, disp := d } =
      s.gpr b + s.gpr i * BitVec.ofNat 64 k + BitVec.ofInt 64 d :=
  rfl

/-- `fs:[base]` -/
theorem ea_fs_base (b : Reg) : s.ea { base := b, seg := some .fs } = s.fsBase + s.gpr b := by
  simp [State.ea, State.offset]

/-- `fs:[base + disp]` -/
theorem ea_fs_base_disp (b : Reg) (d : Int) :
    s.ea { base := b, disp := d, seg := some .fs } = s.fsBase + (s.gpr b + BitVec.ofInt 64 d) :=
  rfl

/-- `fs:[base + index * scale + disp]` -/
theorem ea_fs_index (b i : Reg) (k : Nat) (d : Int) :
    s.ea { base := b, index := some i, scale := k, disp := d, seg := some .fs } =
      s.fsBase + (s.gpr b + s.gpr i * BitVec.ofNat 64 k + BitVec.ofInt 64 d) :=
  rfl

/-! ## The state after a store

A store's state is `{ s with mem := … }`, whose other fields `simp` reads by
projection; these say so for the fields proofs ask about. -/

theorem gpr_withMem (m : Mem) : ({ s with mem := m } : State).gpr = s.gpr := rfl
theorem fsBase_withMem (m : Mem) : ({ s with mem := m } : State).fsBase = s.fsBase := rfl
theorem rd_withMem (m : Mem) : ({ s with mem := m } : State).rd = s.rd := rfl
theorem wr_withMem (m : Mem) : ({ s with mem := m } : State).wr = s.wr := rfl
theorem mem_withMem (m : Mem) : ({ s with mem := m } : State).mem = m := rfl

end VG.X86_64.Access
