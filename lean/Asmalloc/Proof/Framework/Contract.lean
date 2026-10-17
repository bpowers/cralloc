module

public import Lean.Meta.Eqns
public meta import Lean.Meta.Eqns
public meta import Lean.Elab.Command
public import Asmalloc.Proof.Framework.Mem
public import Asmalloc.Proof.Framework.Sig

/-!
# Moving a proof from one contract to a stronger one

A proof may be written against a contract of its own, with its facts spelled
out for the target; the artifact is emitted with the shared contract of
`Spec/`, built with `Sig.contract`. `Contract.Implies k k'` says that `k'`
asks no more of the code than `k` does, so a proof of `Verified T c k` gives
`Verified T c k'` (`Verified.of_implies`), and the correctness of `c` under
`k` gives `Verified T c k'` (`Verified.of_correct`).

`sig_implies` proves `Contract.Implies k k'` cheaply, with the tactics of
`Proof/Framework/Sig.lean`: prefer it to `contract_implies`, which searches
with `simp_all`.
-/

@[expose] public section

namespace VG

/-- `k'` is at least as strong as `k` for its callers: its precondition implies
that of `k`, the postcondition of `k` implies that of `k'`, and `k'` is
satisfiable. -/
structure Contract.Implies {M : ISA} (k k' : Contract M) : Prop where
  pre : ∀ s, k'.pre s → k.pre s
  post : ∀ s s', k'.pre s → k.post s s' → k'.post s s'
  sat : ∃ s, k'.pre s
  abort : ∀ s p, k'.pre s → k.abort s p → k'.abort s p := by intros; trivial

/-- `k` is satisfiable if a contract it implies is. -/
theorem Contract.Implies.sat_left {M : ISA} {k k' : Contract M} (h : k.Implies k') : ∃ s, k.pre s :=
  h.sat.elim fun s hs => ⟨s, h.pre s hs⟩

/-- A satisfiable contract implies itself. -/
theorem Contract.Implies.refl {M : ISA} {k : Contract M} (h : ∃ s, k.pre s) : k.Implies k :=
  ⟨fun _ h => h, fun _ _ _ h => h, h, fun _ _ _ h => h⟩

theorem RseqOk.of_implies {T : Target} {c : Prog T.isa} {k k' : Contract T.isa} {s : T.isa.State}
    (h : RseqOk T c k s) (hk : k.Implies k') (hs : k'.pre s) : RseqOk T c k' s := by
  unfold RseqOk at h ⊢
  split
  · rename_i arm body
    exact ⟨h.1, fun p hp => ⟨(h.2 p hp).1, hk.abort s p hs (h.2 p hp).2⟩⟩
  · trivial

theorem Verified.of_implies {T : Target} {c : Prog T.isa} {k k' : Contract T.isa}
    (h : Verified T c k) (hk : k.Implies k') : Verified T c k' := by
  obtain ⟨hc, hr, -⟩ := h
  refine ⟨fun s hs => ?_, fun s hs => (hr s (hk.pre s hs)).of_implies hk hs, hk.sat⟩
  obtain ⟨s', he, ha, hp⟩ := hc s (hk.pre s hs)
  exact ⟨s', he, ha, hk.post s s' hs hp⟩

/-- Code without a restartable sequence has nothing to check where one
could be aborted. -/
theorem RseqOk.of_ne {T : Target} {c : Prog T.isa} {k : Contract T.isa} {s : T.isa.State}
    (h : ∀ arm body, c ≠ .rseq arm body) : RseqOk T c k s := by
  unfold RseqOk; split
  · exact absurd rfl (h _ _)
  · trivial

/-- `Verified` from the correctness of `c` under a contract `k` that `k'`
implies: the proof of a function against a contract of its own, moved to its
shared contract. `hr`, for code with no restartable sequence, is proved by
default. -/
theorem Verified.of_correct {T : Target} {c : Prog T.isa} {k k' : Contract T.isa}
    (hc : ∀ s, k.pre s → ∃ s', Exec T.isa c s s' ∧ T.abiPreserved s s' ∧ k.post s s')
    (hk : k.Implies k')
    (hr : ∀ s, k'.pre s → RseqOk T c k' s := by exact fun _ _ => trivial) :
    Verified T c k' := by
  refine ⟨fun s hs => ?_, hr, hk.sat⟩
  obtain ⟨s', he, ha, hp⟩ := hc s (hk.pre s hs)
  exact ⟨s', he, ha, hk.post s s' hs hp⟩

theorem Region.disjoint_comm {a b : Region} : a.Disjoint b ↔ b.Disjoint a :=
  ⟨Region.Disjoint.symm, Region.Disjoint.symm⟩

/-- Regions that do not wrap around and lie one after the other are disjoint:
for regions at literal addresses, the hypotheses are closed by `decide`. -/
theorem Region.disjoint_of_le {r₁ r₂ : Region}
    (h : r₁.base.toNat + r₁.len ≤ r₂.base.toNat ∨ r₂.base.toNat + r₂.len ≤ r₁.base.toNat)
    (h₁ : r₁.base.toNat + r₁.len ≤ 2 ^ 64) (h₂ : r₂.base.toNat + r₂.len ≤ 2 ^ 64) : r₁.Disjoint r₂ := by
  intro a c₁ c₂
  simp only [Region.Contains] at c₁ c₂
  bv_omega

open Lean Meta Elab Command in
/-- Generates the equation lemmas of the definitions `ids` in this module, so
that the modules importing it find them, rather than each generating them
again (a fraction of a second each) when `simp` first unfolds a definition. -/
elab "realize_eqns " ids:ident* : command => liftTermElabM do
  for id in ids do
    let n ← realizeGlobalConstNoOverloadWithInfo id
    discard <| getEqnsFor? n
    discard <| getUnfoldEqnFor? n (nonRec := true)

realize_eqns Sig.bufs Curry.apply Curry.const Elem.size ArgWord.ofRaw Sig.contract Sig.words
  Param.words ArgWord.bits IntTy.bits Sig.retBits stackBelow Sig.conj

/-- Two regions that do not wrap around the end of the address space, one
entirely below the other: a check that `decide` evaluates on concrete regions. -/
def Region.sep (a b : Region) : Bool :=
  a.base.toNat + a.len ≤ 2 ^ 64 && b.base.toNat + b.len ≤ 2 ^ 64 &&
    (a.base.toNat + a.len ≤ b.base.toNat || b.base.toNat + b.len ≤ a.base.toNat)

theorem Region.disjoint_of_sep {a b : Region} (h : Region.sep a b = true) : a.Disjoint b := by
  simp only [Region.sep, Bool.and_eq_true, Bool.or_eq_true, decide_eq_true_eq] at h
  exact Region.disjoint_of_le h.2 h.1.1 h.1.2

/-! ## Satisfiability by evaluation

A witness of a contract built with `Sig.contract` is a concrete state, on
which all of the precondition but `A.wf` and the further precondition is a
computation: `Sig.check` computes it (disjointness by `Region.sep`), for the
kernel to evaluate with `decide +kernel`, rather than evaluating each fact
symbolically and closing it by evaluation again. -/

/-- The writable pairs of `l` (those of which one is writable) are separate
(`Region.sep`). -/
def Sig.pairsSep : List (Region × Bool) → Bool
  | [] => true
  | a :: l => l.all (fun b => !(a.2 || b.2) || Region.sep a.1 b.1) && Sig.pairsSep l

theorem Sig.pairwise_of_pairsSep :
    ∀ {l : List (Region × Bool)}, Sig.pairsSep l = true →
      l.Pairwise (fun a b => (a.2 || b.2) = true → a.1.Disjoint b.1)
  | [], _ => .nil
  | a :: l, h => by
    simp only [Sig.pairsSep, Bool.and_eq_true, List.all_eq_true] at h
    refine .cons (fun b hb hab => Region.disjoint_of_sep ?_) (Sig.pairwise_of_pairsSep h.2)
    have := h.1 b hb
    rw [hab] at this
    exact this

/-- All of the precondition of `sig.contract A pre post writeArgs stack` at
`s` but `A.wf` and `pre`, with disjointness decided by `Region.sep`. -/
def Sig.check {M : ISA} (A : Abi M) (sig : Sig) (writeArgs : Bool) (stack : Nat) (s : M.State) :
    Bool :=
  let widths := (sig.words A.ptrBits).map (·.bits A.ptrBits)
  match A.args widths with
  | none => false
  | some vals =>
    let bufs := Sig.bufs sig.params (vals s)
    let all : List (Region × Bool) :=
      bufs ++ (A.argArea widths s).map fun (r, w) => (r, w && writeArgs)
    decide (A.rd s = (all.filter (!·.2)).map (·.1)) &&
      decide (A.wr s = (all.filter (·.2)).map (·.1)) && Sig.pairsSep all &&
      (A.reserved stack s).all (fun r => all.all fun a => Region.sep r a.1) &&
      bufs.all (fun a => decide (a.1.base.toNat + a.1.len ≤ 2 ^ A.ptrBits))

/-- The rest of the precondition of `sig.contract A pre post writeArgs stack`
at `s`: `A.wf` and `pre`. -/
def Sig.wfPre {M : ISA} (A : Abi M) (sig : Sig) (pre : Curry (sig.words A.ptrBits) (Mem → Prop))
    (stack : Nat) (s : M.State) : Prop :=
  let widths := (sig.words A.ptrBits).map (·.bits A.ptrBits)
  match A.args widths with
  | none => False
  | some vals => A.wf widths stack s ∧ Curry.apply (sig.words A.ptrBits) pre (vals s) (A.mem s)

theorem Sig.contract_pre_of_check {M : ISA} {A : Abi M} {sig : Sig}
    {pre : Curry (sig.words A.ptrBits) (Mem → Prop)} {post : sig.Post A.ptrBits} {writeArgs : Bool}
    {stack : Nat} {s : M.State}
    (hc : Sig.check A sig writeArgs stack s = true) (h : Sig.wfPre A sig pre stack s) :
    (sig.contract A pre post writeArgs stack).pre s := by
  simp only [Sig.check] at hc
  simp only [Sig.wfPre] at h
  simp only [Sig.contract]
  split at hc
  · exact absurd hc Bool.false_ne_true
  · rename_i vals hv
    rw [hv] at h ⊢
    simp only [Bool.and_eq_true, decide_eq_true_eq, List.all_eq_true] at hc
    obtain ⟨⟨⟨⟨hrd, hwr⟩, hp⟩, hr⟩, hb⟩ := hc
    exact ⟨h.1, hrd, hwr, Sig.pairwise_of_pairsSep hp,
      fun r hr' a ha => Region.disjoint_of_sep (hr r hr' a ha), hb, h.2⟩

/-- Proves `(sig.contract A …).pre w` (unfolded by `ls`, which `ws` extends to
evaluate the witness `w`) by evaluating `Sig.check` in the kernel; `A.wf` and
the further precondition are evaluated with `sig_reduce` and closed by
evaluation. -/
syntax "sig_sat_check " "[" Lean.Parser.Tactic.simpLemma,* "]" : tactic
macro_rules
  | `(tactic| sig_sat_check [$ls,*]) => `(tactic| (
      sig_apply_check
      · decide +kernel
      · sig_reduce [$ls,*]
        sig_and_intros
        all_goals first
          | trivial
          | decide +kernel))

/-! ## Tactics

Each takes the definitions to unfold: the contracts, the signature and the
calling convention (and its helpers). -/

/-- Proves `∀ s, k'.pre s → k.pre s`, for `k'` built with `Sig.contract`. The
hypothesis is split into its facts once, and each fact of `k.pre` is closed from
them directly (up to the symmetry of `Region.Disjoint`, or by `omega`); only
what that leaves goes to `simp_all`, which simplifies every hypothesis again
for every goal. -/
syntax "implies_pre " "[" Lean.Parser.Tactic.simpLemma,* "]" : tactic
macro_rules
  | `(tactic| implies_pre [$ls,*]) => `(tactic| (
      intro s h
      sig_pre [$ls,*] at h
      sig_split h
      sig_reduce [$ls,*]
      sig_simp [$ls,*] []
      sig_and_intros
      sig_close
      all_goals first
        | with_reducible assumption
        | sig_disjoint_symm
        | omega
        | simp_all [Region.disjoint_comm, Nat.mul_comm]))

/-- Proves `∀ s s', k'.pre s → k.post s s' → k'.post s s'`, for `k'` built with
`Sig.contract`. -/
syntax "implies_post " "[" Lean.Parser.Tactic.simpLemma,* "]" : tactic
macro_rules
  | `(tactic| implies_post [$ls,*]) => `(tactic| (
      intro s s' _ h
      sig_post [$ls,*]
      sig_reduce [$ls,*] at h
      sig_simp [$ls,*] [] at h
      all_goals first
        | exact h
        | simpa [Nat.mul_comm] using h))

/-- Proves `∃ s, k'.pre s` with the witness `w`, for `k'` built with `Sig.contract`
(`ws` unfolds `w`, if `sig_sat_check` fails). -/
syntax "implies_sat " "[" Lean.Parser.Tactic.simpLemma,* "]" " [" Lean.Parser.Tactic.simpLemma,* "]"
  " using " term : tactic
macro_rules
  | `(tactic| implies_sat [$ls,*] [$ws,*] using $w) => `(tactic| (
      refine ⟨$w, ?_⟩
      first
        | sig_sat_check [$ls,*]
        | (sig_pre [$ls,*, $ws,*]
           sig_and_intros
           all_goals first
             | rfl
             | decide
             | exact Region.disjoint_of_sep (by decide)
             | exact Region.disjoint_of_le (by decide) (by decide) (by decide)
             | (intro a h₁ h₂; simp only [Region.Contains] at h₁ h₂; bv_omega))))

/-- Proves `k.Implies k'` for `k'` built with `Sig.contract`, with the witness
`w` for the satisfiability of `k'.pre` (and further definitions to unfold to
evaluate `k'.pre` on it). -/
syntax "contract_implies " "[" Lean.Parser.Tactic.simpLemma,* "]"
  " [" Lean.Parser.Tactic.simpLemma,* "]" " using " term : tactic
macro_rules
  | `(tactic| contract_implies [$ls,*] [$ws,*] using $w) => `(tactic| exact
      { pre := by implies_pre [$ls,*]
        post := by implies_post [$ls,*]
        sat := by implies_sat [$ls,*] [$ws,*] using $w })

/-! ## Cheap implications

`sig_implies [ls] [ws] using w` proves `k.Implies k'` for `k'` built with
`Sig.contract` and `k` (unfolded by `ls`: both contracts, the signature and
the calling convention) a contract whose precondition is a conjunction of facts
that each follow from one of `k'`'s (as `sig_pre` evaluates it) by
`assumption`, by symmetry of `Region.Disjoint` or by `omega`, and whose
postcondition is `k'`'s as `sig_post` evaluates it. `w` is a state
satisfying `k'.pre`, and `ws` unfolds what `k'.pre` needs to evaluate on it.
Unlike `contract_implies`, nothing searches over all hypotheses with `simp`. -/

/-- Proves `∀ s, k'.pre s → k.pre s` (see `sig_implies`). -/
syntax "sig_implies_pre " "[" Lean.Parser.Tactic.simpLemma,* "]" : tactic
macro_rules
  | `(tactic| sig_implies_pre [$ls,*]) => `(tactic| (
      intro s h
      sig_pre [$ls,*] at h
      sig_split h
      sig_reduce [$ls,*]
      sig_simp [$ls,*] []
      sig_and_intros
      sig_close
      all_goals first
        | with_reducible assumption
        | sig_disjoint_symm
        | omega
        | (sig_simp_only [Nat.mul_comm] at *
           first | with_reducible assumption | sig_disjoint_symm)
        | simp only [*, List.mem_cons, List.mem_singleton, true_or, or_true]))

/-- Proves `∀ s s', k'.pre s → k.post s s' → k'.post s s'` (see `sig_implies`), or
`… → (P → k'.post s s')` for a postcondition guarded by a fact `P`. -/
syntax "sig_implies_post " "[" Lean.Parser.Tactic.simpLemma,* "]" : tactic
macro_rules
  | `(tactic| sig_implies_post [$ls,*]) => `(tactic| (
      intro s s' _ h
      sig_post [$ls,*]
      sig_reduce [$ls,*] at h
      sig_simp [$ls,*] [] at h
      -- A postcondition may restate a hypothesis of the precondition.
      first | exact h | exact fun _ => h))

/-- Proves `∃ s, k'.pre s` with the witness `w` (see `sig_sat_check`; `ws`
unfolds `w` if that fails). -/
syntax "sig_implies_sat " "[" Lean.Parser.Tactic.simpLemma,* "]" " [" Lean.Parser.Tactic.simpLemma,* "]"
  " using " term : tactic
macro_rules
  | `(tactic| sig_implies_sat [$ls,*] [$ws,*] using $w) => `(tactic| (
      refine ⟨$w, ?_⟩
      first
        | sig_sat_check [$ls,*]
        | (sig_pre [$ls,*]
           sig_and_intros
           all_goals first
             | rfl
             | decide
             | exact Region.disjoint_of_sep (by decide)
             | (intro a h₁ h₂
                set_option linter.unusedSimpArgs false in
                sig_simp_only [Region.Contains, $ws,*] at h₁ h₂
                bv_omega))))

syntax "sig_implies " "[" Lean.Parser.Tactic.simpLemma,* "]" " [" Lean.Parser.Tactic.simpLemma,* "]"
  " using " term : tactic
macro_rules
  | `(tactic| sig_implies [$ls,*] [$ws,*] using $w) => `(tactic| exact
      { pre := by sig_implies_pre [$ls,*]
        post := by sig_implies_post [$ls,*]
        sat := by sig_implies_sat [$ls,*] [$ws,*] using $w })

end VG
