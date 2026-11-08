module

public import Cralloc.Proof.CEntry.AArch64.Malloc
public import Cralloc.Proof.CEntry.AArch64.Witness

/-!
# `c_malloc`'s witnesses on AArch64

`C_SIZES` gives the size 8 (index 1) the class 1, and a magazine at `0x4000` has 4 slots, whose slot 1
holds `0x5000`. One state per way out: a CPU number with no slab, a size above 32768, an empty slab
word, and a pop.
-/

@[expose] public section

namespace VG.Proof.CEntry.AArch64.Malloc

open VG.AArch64 VG.Spec.Slab VG.Spec.CEntry VG.Impl.Slab.AArch64 VG.Impl.CEntry.AArch64
  VG.Proof.Slab.AArch64 VG.Proof.CEntry VG.Proof.CEntry.AArch64 VG.Proof.Slab

def wsOf (cpu word : BitVec 64) : List (Addr × BitVec 64) :=
  [(0x2000, 0x100), (0x10104, cpu), (0x1000008, word), (0x4008, 0x5000), (0x3000808, 16),
    (0x8000800, 0x100)]

def witM (cpu size word : BitVec 64) (rd wr : List Region) : State :=
  witS SIZES size 0 0 0x8000800 (wsOf cpu word) rd wr

def roM : List Region := [⟨0x2000, 8⟩, ⟨0x10104, 4⟩, ⟨0x3000800, 2048⟩, ⟨0x8000800, 4097⟩]

def w0 : State := witM 8192 8 0 roM [⟨0x10108, 8⟩]
def w1 : State := witM 0 40000 0 roM [⟨0x10108, 8⟩]
def w2 : State := witM 0 8 0 roM [⟨0x10108, 8⟩, ⟨0x1000008, 8⟩]
def w3 : State := witM 0 8 0x4010 roM [⟨0x10108, 8⟩, ⟨0x1000008, 8⟩, ⟨0x4000, 32⟩]

theorem pre_w0 : attempt.pre w0 :=
  ⟨pre_of_check hX (ws := wsOf 8192 0) rfl rfl hv (fun _ => rfl) (fun _ => rfl) (by decide +kernel),
    ⟨by decide +kernel, by decide +kernel⟩, by decide +kernel, by decide +kernel, by decide +kernel,
    by decide +kernel⟩

theorem pre_w1 : attempt.pre w1 :=
  ⟨pre_of_check hX (ws := wsOf 0 0) rfl rfl hv (fun _ => rfl) (fun _ => rfl) (by decide +kernel),
    ⟨by decide +kernel, by decide +kernel⟩, by decide +kernel, by decide +kernel, by decide +kernel,
    by decide +kernel⟩

theorem pre_w2 : attempt.pre w2 :=
  ⟨pre_of_check hX (ws := wsOf 0 0) rfl rfl hv (fun _ => rfl) (fun _ => rfl) (by decide +kernel),
    ⟨by decide +kernel, by decide +kernel⟩, by decide +kernel, by decide +kernel, by decide +kernel,
    by decide +kernel⟩

theorem pre_w3 : attempt.pre w3 :=
  ⟨pre_of_check hX (ws := wsOf 0 0x4010) rfl rfl hv (fun _ => rfl) (fun _ => rfl) (by decide +kernel),
    ⟨by decide +kernel, by decide +kernel⟩, by decide +kernel, by decide +kernel, by decide +kernel,
    by decide +kernel⟩

theorem outcomes_eq : cMalloc.outcomes = [(0, true), (1, true), (2, true), (3, true)] := rfl

theorem visits_w0 : Visits isa cMalloc w0 0 true := by
  have hp := pre_of pre_w0
  exact .seqL (Visits.rseq (execBlock_arm hp.arm) (xC hp (by decide +kernel)).stop0)

theorem visits_w1 : Visits isa cMalloc w1 1 true := by
  have hp := pre_of pre_w1
  exact .seqL (Visits.rseq (execBlock_arm hp.arm) (xS hp (by decide +kernel) (by decide +kernel)).stop0)

theorem visits_w2 : Visits isa cMalloc w2 2 true := by
  have hp := pre_of pre_w2
  exact .seqL (Visits.rseq (execBlock_arm hp.arm)
    (xP hp (by decide +kernel) (by decide +kernel) (by decide +kernel)).stop0)

theorem visits_w3 : Visits isa cMalloc w3 3 true := by
  have hp := pre_of pre_w3
  exact .seqL (Visits.rseq (execBlock_arm hp.arm)
    (tP hp (by decide +kernel) (by decide +kernel) (by decide +kernel)).stop0)

def witnesses : List State := [w0, w1, w2, w3]

theorem witnessOk : (∃ w ∈ witnesses, attempt.pre w) ∧
    ∀ o ∈ cMalloc.outcomes, ∃ w ∈ witnesses, attempt.pre w ∧ Visits isa cMalloc w o.1 o.2 := by
  refine ⟨⟨w0, by simp [witnesses], pre_w0⟩, fun o ho => ?_⟩
  rw [outcomes_eq] at ho
  simp only [List.mem_cons, List.not_mem_nil, or_false] at ho
  rcases ho with rfl | rfl | rfl | rfl
  · exact ⟨w0, by simp [witnesses], pre_w0, visits_w0⟩
  · exact ⟨w1, by simp [witnesses], pre_w1, visits_w1⟩
  · exact ⟨w2, by simp [witnesses], pre_w2, visits_w2⟩
  · exact ⟨w3, by simp [witnesses], pre_w3, visits_w3⟩

theorem verified : Verified target cMalloc attempt :=
  ⟨fun _ hs => correct hs, fun _ hs => rseqOk hs, ⟨_, pre_w0⟩, fun o ho => by
    obtain ⟨w, -, hw, hv⟩ := witnessOk.2 o ho
    exact ⟨w, hw, hv⟩⟩

/-- The last attempt, aborted any number of times, starts with the callee-saved registers, the stack
pointer and the argument of the first. -/
theorem restartsOk : ∀ s s', attempt.pre s → Restarts target cMalloc attempt s s' →
    target.abiPreserved s s' ∧ ArgsKept target.abi mallocSig s s' ∧
    (target.inline = true → target.preservedExcept [] s s') :=
  restartsOk_target verified (by decide) fun _ hs p hp r hr =>
    (mids (pre_of hs) p hp).gpr r (by revert r; decide)

end VG.Proof.CEntry.AArch64.Malloc
