module

public import Cralloc.Proof.CEntry.AArch64.Back
public import Cralloc.Proof.Framework.AArch64.Retry

/-!
# The C routines' common steps on AArch64

The stretches every routine has before its back half (the CPU number, its slab, a table's byte),
and the accesses a contract's trace states, as the steps make them.
-/

@[expose] public section

namespace VG.Proof.CEntry.AArch64

open VG.AArch64 VG.Spec.Slab VG.Spec.CEntry VG.Impl.Slab.AArch64 VG.Impl.CEntry.AArch64
  VG.Proof.Slab.AArch64 VG.Proof.CEntry VG.Proof.Slab

/-! ## The CPU number -/

section Cpu

variable {s : State}

/-- A CPU number with a slab. -/
theorem cpuT (h : ArmOk s) (hi : (cpu32 s).toNat < 8192) :
    Through isa cpuC (σA s) (cpu2 (σA s) s) [σA s, cpu1 (σA s) s, cpu2 (σA s) s]
      [⟨area s + 4, 4, false, (cpu32 s).toNat⟩] :=
  have hb : Base s (σA s) := ⟨rfl, rfl, rfl, h.ready.1⟩
  have he : eval .hs (cpu2 (σA s) s) = some false := by
    rw [eval_cpu]; simp only [Option.some.injEq, decide_eq_false_iff_not]; omega
  .mem (exec_cpu h hb) (ev_cpu hb) <| .reg exec_cmp rfl <| .skip he <| .nil _

/-- A CPU number without one. -/
theorem cpuX (h : ArmOk s) (hi : ¬ (cpu32 s).toNat < 8192) :
    Leaves isa cpuC (σA s) (.exit "slow" (cpu2 (σA s) s)) [σA s, cpu1 (σA s) s, cpu2 (σA s) s]
      [⟨area s + 4, 4, false, (cpu32 s).toNat⟩] 2 :=
  have hb : Base s (σA s) := ⟨rfl, rfl, rfl, h.ready.1⟩
  have he : eval .hs (cpu2 (σA s) s) = some true := by
    rw [eval_cpu]; simp only [Option.some.injEq, decide_eq_true_eq]; omega
  .mem (exec_cpu h hb) (ev_cpu hb) <| .reg exec_cmp rfl <| .take he

end Cpu

/-! ## The CPU's slab -/

section Slab

variable {s τ : State}

def slab1 (τ s : State) : State := τ.setReg .x8 (sh s)
def slab2 (τ s : State) : State := (slab1 τ s).setReg .x3 (s.syms SLABS + sh s)

theorem slabT (h8 : τ.gpr .x8 = (cpu32 s).setWidth 64) (h13 : τ.gpr .x13 = s.syms SLABS) :
    Through isa slabOf τ (slab2 τ s) [τ, slab1 τ s] [] :=
  have e1 : exec (.lsl .x .x8 .x8 12) τ = some (slab1 τ s) := by
    show some (τ.setReg .x8 (τ.gpr .x8 <<< 12)) = _
    rw [h8]
    rfl
  have e2 : exec (.add .x .x3 .x13 (.reg .x8)) (slab1 τ s) = some (slab2 τ s) := by
    show some ((slab1 τ s).setReg .x3 ((slab1 τ s).gpr .x13 + (slab1 τ s).gpr .x8)) = _
    rw [slab1, RegUpd.gpr_setReg_of_ne _ _ (by decide), RegUpd.gpr_setReg_self, h13]
    rfl
  .reg e1 rfl <| .reg e2 rfl <| .nil _

theorem mid_slab1 {rs : List Reg} {m : Mem} (h : Mid rs s m τ) (h8 : .x8 ∈ rs) :
    Mid rs s m (slab1 τ s) := h.setReg h8 _
theorem mid_slab2 {rs : List Reg} {m : Mem} (h : Mid rs s m τ) (h8 : .x8 ∈ rs) (h3 : .x3 ∈ rs) :
    Mid rs s m (slab2 τ s) := (mid_slab1 h h8).setReg h3 _

theorem slab2_x3 : (slab2 τ s).gpr .x3 = s.syms SLABS + sh s := RegUpd.gpr_setReg_self _ _ _
theorem slab2_gpr {r : Reg} (h8 : ¬ r = .x8) (h3 : ¬ r = .x3) : (slab2 τ s).gpr r = τ.gpr r := by
  rw [slab2, RegUpd.gpr_setReg_of_ne _ _ h3, slab1, RegUpd.gpr_setReg_of_ne _ _ h8]

end Slab

/-! ## A table's byte -/

section Byte

variable {s σ : State} {a : Addr} {t : Reg} {m : MemOp}

/-- A byte of a table, in memory as after the arm. -/
structure ByteOk (s : State) (a : Addr) : Prop where
  rd : InRegions (s.rd ++ s.wr) a 1
  cs : Region.Disjoint ⟨a, 1⟩ ⟨area s + 8, 8⟩

theorem byte_ok {regs : List Reg} {c : Spec.CEntry.Ctx isa} (hA : A64 c.A regs) (hw : c.World s)
    (hl : c.Layout s) {r : Region} (hm : (r, false) ∈ tables c s) (hc : r.Contains a 1) : ByteOk s a := by
  have hin : r ∈ s.rd := mem_rd hA hw (x := (r, false)) (mem_fixed (table_mem_fixed hm)) rfl
  have hd : (c.csField s).Disjoint r := cs_table hl (x := (r, false)) (List.mem_cons_of_mem _ hm)
  rw [csField_c hA] at hd
  exact ⟨(InRegions.of_mem hin hc).append_left, (hd.sub_right (sub_of_contains hc)).symm⟩

theorem ByteOk.armed (h : ByteOk s a) : (m1 s) a = s.mem a :=
  Mem.write_apply (h.cs.sep (Region.contains_self _ _) (Region.contains_self _ _) a (by simp))

theorem exec_byte {rs : List Reg} (h : ByteOk s a) (hb : Mid rs s (m1 s) σ) (hea : σ.ea m = a) :
    exec (.ldr .b t m) σ = some (σ.setReg t ((s.mem a).setWidth 64)) := by
  rw [Access.exec_ldr_b (by rw [hea, hb.rd, hb.wr]; exact h.rd), hea, hb.mem, h.armed]

theorem ev_byte {rs : List Reg} (h : ByteOk s a) (hb : Mid rs s (m1 s) σ) (hea : σ.ea m = a) :
    Instr.events (.ldr .b t m) σ (σ.setReg t ((s.mem a).setWidth 64)) =
      [⟨a, 1, false, (s.mem a).toNat⟩] := by
  simp only [Instr.events, hea, Width.bytes, hb.mem, Mem.read, h.armed]
  rw [BitVec.toNat_append]
  simp

end Byte

/-! ## The accesses, as a contract states them -/

theorem ofNat_toNat64 (v : BitVec 64) : BitVec.ofNat 64 v.toNat = v := by
  rw [BitVec.ofNat_toNat, BitVec.setWidth_eq]

theorem q32 (v : BitVec 64) : BitVec.ofNat 32 (v.toNat + 8) = (v + 8).setWidth 32 := by
  apply BitVec.eq_of_toNat_eq
  rw [BitVec.toNat_ofNat, BitVec.toNat_setWidth, add8_toNat_mod,
    Nat.mod_mod_of_dvd _ (by decide : 2 ^ 32 ∣ 2 ^ 64)]

section Trace

variable {regs : List Reg} {c : Spec.CEntry.Ctx isa} (hA : A64 c.A regs) {s τ : State} {k : Nat}
include hA

theorem head_eq : c.head s (o s).toNat s.csDesc.toNat (cpu32 s).toNat =
    [⟨s.syms RS, 8, false, (o s).toNat⟩, ⟨area s + 8, 8, true, s.csDesc.toNat⟩,
      ⟨area s + 4, 4, false, (cpu32 s).toNat⟩] := by
  simp only [Spec.CEntry.Ctx.head, Spec.CEntry.Ctx.rd, Spec.CEntry.Ctx.wr, tp_of (c := c.slab) hA,
    sym_c hA, ofNat_toNat64]
  rfl

variable (hr : Ready s k τ)
include hr

theorem word_eq : wordAddr (c.sym s SLABS) (cpu32 s).toNat k = Tail.wa τ := by
  rw [wa_eq hA hr, Spec.CEntry.Ctx.word, cpu_c hA]

/-- A pop's accesses that found nothing to give. -/
theorem popTail_miss (ht : Tail.ptst τ = 0) (v : Nat) :
    c.popTail s (cpu32 s).toNat k (Tail.wv τ).toNat (Tail.mk τ).toNat v =
      [⟨Tail.wa τ, 8, false, (Tail.wv τ).toNat⟩, ⟨Tail.ma τ, c.A.maskBytes, false, (Tail.mk τ).toNat⟩] := by
  have hz : BitVec.ofNat 32 (Tail.wv τ).toNat &&& BitVec.ofNat 32 (Tail.mk τ).toNat = 0 := by
    rw [BitVec.ofNat_toNat, BitVec.ofNat_toNat]; exact ht
  simp only [Spec.CEntry.Ctx.popTail, Spec.CEntry.Ctx.rd, hz, ne_eq, not_true_eq_false, ↓reduceIte,
    List.append_nil, word_eq hA hr, ← ma_eq hA hr]

/-- A pop's accesses that took a pointer. -/
theorem popTail_hit (ht : ¬ Tail.ptst τ = 0) :
    c.popTail s (cpu32 s).toNat k (Tail.wv τ).toNat (Tail.mk τ).toNat (Tail.px τ).toNat =
      [⟨Tail.wa τ, 8, false, (Tail.wv τ).toNat⟩, ⟨Tail.ma τ, c.A.maskBytes, false, (Tail.mk τ).toNat⟩,
        ⟨Tail.wv τ - 8, 8, false, (Tail.px τ).toNat⟩, ⟨Tail.wa τ, 8, true, (Tail.wv τ - 8).toNat⟩] := by
  have hz : ¬ BitVec.ofNat 32 (Tail.wv τ).toNat &&& BitVec.ofNat 32 (Tail.mk τ).toNat = 0 := by
    rw [BitVec.ofNat_toNat, BitVec.ofNat_toNat]; exact ht
  simp only [Spec.CEntry.Ctx.popTail, Spec.CEntry.Ctx.rd, Spec.CEntry.Ctx.wr, hz, ne_eq, not_false_eq_true,
    ↓reduceIte, word_eq hA hr, ← ma_eq hA hr, ofNat_toNat64, List.cons_append, List.nil_append]

/-- A push's accesses that found no free slot to spare. -/
theorem pushTail_miss (ht : Tail.qtst τ = 0) (x : BitVec 64) :
    c.pushTail s x (cpu32 s).toNat k (Tail.wv τ).toNat (Tail.mk τ).toNat =
      [⟨Tail.wa τ, 8, false, (Tail.wv τ).toNat⟩, ⟨Tail.ma τ, c.A.maskBytes, false, (Tail.mk τ).toNat⟩] := by
  have hz : BitVec.ofNat 32 ((Tail.wv τ).toNat + 8) &&& BitVec.ofNat 32 (Tail.mk τ).toNat = 0 := by
    rw [q32, BitVec.ofNat_toNat]; exact ht
  simp only [Spec.CEntry.Ctx.pushTail, Spec.CEntry.Ctx.rd, hz, ne_eq, not_true_eq_false, ↓reduceIte,
    List.append_nil, word_eq hA hr, ← ma_eq hA hr]

/-- A push's accesses that stored its pointer. -/
theorem pushTail_hit (ht : ¬ Tail.qtst τ = 0) (x : BitVec 64) :
    c.pushTail s x (cpu32 s).toNat k (Tail.wv τ).toNat (Tail.mk τ).toNat =
      [⟨Tail.wa τ, 8, false, (Tail.wv τ).toNat⟩, ⟨Tail.ma τ, c.A.maskBytes, false, (Tail.mk τ).toNat⟩,
        ⟨Tail.wv τ, 8, true, x.toNat⟩, ⟨Tail.wa τ, 8, true, (Tail.wv τ + 8).toNat⟩] := by
  have hz : ¬ BitVec.ofNat 32 ((Tail.wv τ).toNat + 8) &&& BitVec.ofNat 32 (Tail.mk τ).toNat = 0 := by
    rw [q32, BitVec.ofNat_toNat]; exact ht
  simp only [Spec.CEntry.Ctx.pushTail, Spec.CEntry.Ctx.rd, Spec.CEntry.Ctx.wr, hz, ne_eq, not_false_eq_true,
    ↓reduceIte, word_eq hA hr, ← ma_eq hA hr, ofNat_toNat64, List.cons_append, List.nil_append]

end Trace

end VG.Proof.CEntry.AArch64
