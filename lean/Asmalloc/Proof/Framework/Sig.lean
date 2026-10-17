module

public import Asmalloc.Proof.Framework.SigEval
public import Asmalloc.Proof.Framework.PlainSimp
public meta import Asmalloc.Proof.Framework.PlainSimp
public import Asmalloc.TCB.Artifact

/-!
# Proving code against a contract built with `Sig.contract`

The contracts of `Spec/` are built with `Sig.contract` from a signature and a
calling convention. For a concrete signature and calling convention, the
tactics here evaluate the parts of such a contract into the plain facts a
proof works with:

* `sig_pre [defs] at h` turns `h : (sig.contract A …).pre s` into a
  conjunction of `A.wf` (if it says anything), `s.rd = […]`, `s.wr = […]`,
  the disjointness of each pair of buffers of which one is writable (in the
  order of the signature), of the reserved regions (the return address and
  the stack below it, in `A.reserved`'s order) and each buffer, the bounds
  `p.toNat + len ≤ 2 ^ ptrBits` of each buffer, and the further
  precondition; `sig_pre [defs]` on the goal does the same to prove it;
* `sig_post [defs]` turns `(sig.contract A …).post s s'` (a goal or, with
  `at`, a hypothesis) into the postcondition applied to the arguments.

`defs` are the definitions to unfold: the contract, the signature, the
calling convention and its helpers. Each runs one `sig_reduce` and one
`simp only` with a fixed set of lemmas, and no search over
hypotheses.
-/

@[expose] public section

namespace VG

theorem BitVec.toNat_setWidth_32_64 (x : BitVec 32) : (x.setWidth 64).toNat = x.toNat := by
  simp [BitVec.toNat_setWidth]; omega

theorem BitVec.setWidth_32_64_32 (x : BitVec 32) : (x.setWidth 64).setWidth 32 = x := by
  simp

theorem Curry.apply_const {α : Type} (a : α) :
    ∀ (ws : List ArgWord) (vs : List (BitVec 64)), Curry.apply ws (Curry.const a ws) vs = a
  | [], _ => rfl
  | _ :: ws, [] => Curry.apply_const a ws []
  | _ :: ws, _ :: vs => Curry.apply_const a ws vs

/-! ## Conjunctions of lists of facts

The disjointness and bounds facts of `Sig.contract` are stated with
`List.Pairwise` and `∀ x ∈ l`, which only rewriting turns into facts. Stated
instead as `Sig.conj` of a list of facts, which evaluation computes, they are
rewritten once, by the lemmas below. -/

/-- The conjunction of `l`. -/
def Sig.conj : List Prop → Prop
  | [] => True
  | [p] => p
  | p :: q :: l => p ∧ Sig.conj (q :: l)

theorem Sig.conj_cons {p : Prop} {l : List Prop} : Sig.conj (p :: l) ↔ p ∧ Sig.conj l := by
  cases l with
  | nil => exact ⟨fun h => ⟨h, trivial⟩, fun h => h.1⟩
  | cons => exact Iff.rfl

theorem Sig.conj_append {l₁ l₂ : List Prop} : Sig.conj (l₁ ++ l₂) ↔ Sig.conj l₁ ∧ Sig.conj l₂ := by
  induction l₁ with
  | nil => exact ⟨fun h => ⟨trivial, h⟩, fun h => h.2⟩
  | cons p l ih => rw [List.cons_append, Sig.conj_cons, Sig.conj_cons, ih, and_assoc]

theorem Sig.conj_map {α : Type} {P : α → Prop} {l : List α} : Sig.conj (l.map P) ↔ ∀ a ∈ l, P a := by
  induction l with
  | nil => exact ⟨fun _ _ h => absurd h List.not_mem_nil, fun _ => trivial⟩
  | cons a l ih => rw [List.map_cons, Sig.conj_cons, ih, List.forall_mem_cons]

/-- The disjointness of each pair of regions of `l` (in order) of which one is writable. -/
def Sig.pairFacts : List (Region × Bool) → List Prop
  | [] => []
  | a :: l => l.filterMap (fun b => bif a.2 || b.2 then some (a.1.Disjoint b.1) else none) ++
      Sig.pairFacts l

theorem Sig.forall_pair_iff {a : Region × Bool} {l : List (Region × Bool)} :
    (∀ b ∈ l, (a.2 || b.2) = true → a.1.Disjoint b.1) ↔
      Sig.conj (l.filterMap fun b => bif a.2 || b.2 then some (a.1.Disjoint b.1) else none) := by
  induction l with
  | nil => exact ⟨fun _ => trivial, fun _ _ h => absurd h List.not_mem_nil⟩
  | cons b l ih =>
    rw [List.forall_mem_cons, ih, List.filterMap_cons]
    cases a.2 || b.2
    · simp only [Bool.cond_false, Bool.false_eq_true, false_implies, true_and]
    · simp only [Bool.cond_true, Sig.conj_cons, true_implies]

theorem Sig.pairwise_iff {l : List (Region × Bool)} :
    l.Pairwise (fun a b => (a.2 || b.2) = true → a.1.Disjoint b.1) ↔ Sig.conj (Sig.pairFacts l) := by
  induction l with
  | nil => exact ⟨fun _ => trivial, fun _ => .nil⟩
  | cons a l ih => rw [List.pairwise_cons, Sig.pairFacts, Sig.conj_append, ih, Sig.forall_pair_iff]

theorem Sig.forall_mem_disjoint_iff {rs : List Region} {l : List (Region × Bool)} :
    (∀ r ∈ rs, ∀ a ∈ l, r.Disjoint a.1) ↔
      Sig.conj (rs.flatMap fun r => l.map fun a => r.Disjoint a.1) := by
  induction rs with
  | nil => exact ⟨fun _ => trivial, fun _ _ h => absurd h List.not_mem_nil⟩
  | cons r rs ih => rw [List.forall_mem_cons, List.flatMap_cons, Sig.conj_append, Sig.conj_map, ih]

theorem Sig.forall_mem_bound_iff {n : Nat} {l : List (Region × Bool)} :
    (∀ a ∈ l, a.1.base.toNat + a.1.len ≤ 2 ^ n) ↔
      Sig.conj (l.map fun a => a.1.base.toNat + a.1.len ≤ 2 ^ n) :=
  Sig.conj_map.symm

/-- The precondition of `sig.contract A pre _ writeArgs stack`, with its
disjointness and bounds facts as `Sig.conj` of lists, which evaluate. -/
def Sig.preE {M : ISA} (A : Abi M) (sig : Sig) (pre : Curry (sig.words A.ptrBits) (Mem → Prop))
    (writeArgs : Bool) (stack : Nat) (s : M.State) : Prop :=
  let ws := sig.words A.ptrBits
  let widths := ws.map (·.bits A.ptrBits)
  match A.args widths with
  | none => False
  | some vals =>
    let bufs := Sig.bufs sig.params (vals s)
    let all := bufs ++ (A.argArea widths s).map fun (r, w) => (r, w && writeArgs)
    A.wf widths stack s ∧
    A.rd s = (all.filter (!·.2)).map (·.1) ∧ A.wr s = (all.filter (·.2)).map (·.1) ∧
    Sig.conj (Sig.pairFacts all) ∧
    Sig.conj ((A.reserved stack s).flatMap fun r => all.map fun a => r.Disjoint a.1) ∧
    Sig.conj (bufs.map fun a => a.1.base.toNat + a.1.len ≤ 2 ^ A.ptrBits) ∧
    Curry.apply ws pre (vals s) (A.mem s)

theorem Sig.contract_pre_iff {M : ISA} {A : Abi M} {sig : Sig}
    {pre : Curry (sig.words A.ptrBits) (Mem → Prop)} {post : sig.Post A.ptrBits} {writeArgs : Bool}
    {stack : Nat} {x : Sig.Extra M} {s : M.State} :
    (sig.contract A pre post writeArgs stack x).pre s ↔ Sig.preE A sig pre writeArgs stack s := by
  simp only [Sig.contract, Sig.preE]
  generalize A.args ((sig.words A.ptrBits).map (·.bits A.ptrBits)) = o
  cases o with
  | none => exact Iff.rfl
  | some vals => simp only [Sig.pairwise_iff, Sig.forall_mem_disjoint_iff, Sig.forall_mem_bound_iff]

/-! ## Tables of constants

`Abi.withConsts` states each table's region `⟨f name, 8 * words.length⟩`
(`Abi.constRegions`) and facts about its base and length. Evaluated by
unfolding `List.map` and by `forall_eq`, these leave the projections of a
`Region` and of a pair, which `simp` reduces definitionally: the kernel then
compares the region's length `8 * words.length` with the projection of it,
unfolds the two sides in step until one is `Nat.mul` on a closed term, and
evaluates `words.length`, the whole table. Rewritten by these lemmas, proven
for any table and not by unfolding, the length is never compared with
another form of itself. -/

/-- `Abi.constRegions` (unfolded) on a list of tables, a table at a time. The
base is any function `F` of the table, which `simp` matches as a pattern. -/
theorem Sig.map_const_cons {F : String × List (BitVec 64) → Addr} {n : String}
    {w : List (BitVec 64)} {cs : List (String × List (BitVec 64))} :
    List.map (fun c : String × List (BitVec 64) => ({ base := F c, len := 8 * c.2.length } : Region))
        ((n, w) :: cs) =
      ⟨F (n, w), 8 * w.length⟩ :: List.map (fun c : String × List (BitVec 64) =>
        ({ base := F c, len := 8 * c.2.length } : Region)) cs := (rfl)

/-- The facts of `Abi.withConsts` about the tables' regions, a region at a time. -/
theorem Sig.forall_mem_const_cons {b : Addr} {n k : Nat} {W R ts : List Region} :
    (∀ t ∈ (⟨b, n⟩ : Region) :: ts,
        t.base.toNat + t.len ≤ 2 ^ k ∧ (∀ r ∈ W, t.Disjoint r) ∧ ∀ r ∈ R, t.Disjoint r) ↔
      (b.toNat + n ≤ 2 ^ k ∧ (∀ r ∈ W, Region.Disjoint ⟨b, n⟩ r) ∧ ∀ r ∈ R, Region.Disjoint ⟨b, n⟩ r) ∧
        ∀ t ∈ ts, t.base.toNat + t.len ≤ 2 ^ k ∧ (∀ r ∈ W, t.Disjoint r) ∧ ∀ r ∈ R, t.Disjoint r :=
  List.forall_mem_cons

/-- `Abi.constRegions`, a table at a time (for proofs that unfold it by hand). -/
theorem Abi.constRegions_cons {f : String → Addr} {n : String} {w : List (BitVec 64)}
    {cs : List (String × List (BitVec 64))} :
    Abi.constRegions f ((n, w) :: cs) = ⟨f n, 8 * w.length⟩ :: Abi.constRegions f cs := (rfl)

theorem Abi.constRegions_nil {f : String → Addr} : Abi.constRegions f [] = [] := (rfl)

/-- A table's region's bound and its separation from the regions `W`, with its
base and length as they are (see `Sig.map_const_cons`). -/
theorem Sig.forall_mem_const_single {b : Addr} {n k : Nat} {W : List Region} :
    (∀ t ∈ [(⟨b, n⟩ : Region)], t.base.toNat + t.len ≤ 2 ^ k ∧ ∀ r ∈ W, t.Disjoint r) ↔
      b.toNat + n ≤ 2 ^ k ∧ ∀ r ∈ W, Region.Disjoint ⟨b, n⟩ r :=
  List.forall_mem_singleton

/-! ## Evaluation

The signature and the calling convention are data: evaluating them (the
argument words, the buffers, the argument area, which buffers are writable)
is computation, which `sig_reduce` (`Proof/Framework/SigEval.lean`) does by
unfolding definitions, with no proof term: the kernel checks the result once,
by evaluation. Only then does `simp` rewrite what is not computation
(`List.Pairwise` and membership in the concrete lists, `setWidth`, the
conjunctions), on a term that is already small. (Evaluating the lists with
`simp` instead produces a proof step per list operation, each stated on the
whole contract, which the kernel spends up to seconds checking.) -/

/-- `sig_simp [ls] [xs] (at h)?` is `simp -failIfUnchanged only [xs, ts]`,
where `ts` are the theorems among `ls` (the definitions among them are
unfolded by `sig_reduce`, which is cheaper than elaborating their equations
into simp lemmas at every use); nothing if there are none. -/
syntax "sig_simp " "[" Lean.Parser.Tactic.simpLemma,* "]" " [" Lean.Parser.Tactic.simpLemma,* "]"
  (Lean.Parser.Tactic.location)? : tactic

open Lean Elab Tactic in
elab_rules : tactic
  | `(tactic| sig_simp [$ls,*] [$xs,*] $[$loc]?) => do
    let mut args : Array (TSyntax ``Lean.Parser.Tactic.simpLemma) := xs.getElems
    for l in ls.getElems do
      let t := l.raw[2]
      unless t.isIdent do continue
      let cs ← try realizeGlobalConstWithInfos t catch _ => pure []
      let env ← getEnv
      -- In a module, a theorem imported from another module is an axiom.
      if cs.any fun c => (env.find? c).any (· matches .thmInfo _ | .axiomInfo _) then args := args.push l
    if args.isEmpty then return
    args ← unhygienicSimpArgs args
    evalTactic (← `(tactic|
      set_option linter.unusedSimpArgs false in simp -failIfUnchanged only [$args,*] $[$loc]?))

/-- `sig_simp_only [xs] (at h)?` is `simp only [xs] (at h)?`, for the simp
calls of macros: their lemma names are elaborated as plain names
(`unhygienicSimpArgs`). -/
syntax "sig_simp_only " "[" Lean.Parser.Tactic.simpLemma,* "]" (Lean.Parser.Tactic.location)? : tactic

open Lean Elab Tactic in
elab_rules : tactic
  | `(tactic| sig_simp_only [$xs,*] $[$loc]?) => do
    let args ← unhygienicSimpArgs xs.getElems
    evalTactic (← `(tactic| simp only [$args,*] $[$loc]?))

/-- Unfolds a contract built with `Sig.contract` (at `loc`), evaluating the
signature and the calling convention given by `ls`. -/
syntax "sig_eval " "[" Lean.Parser.Tactic.simpLemma,* "]" (Lean.Parser.Tactic.location)? : tactic
macro_rules
  | `(tactic| sig_eval [$ls,*] $[$loc]?) => `(tactic| (
      -- The facts about lists as `Sig.conj` of lists, which evaluate.
      first
        | (sig_iff $[$loc]?
           sig_reduce [$ls,*] $[$loc]?)
        | (sig_reduce [$ls,*] $[$loc]?
           try sig_simp_only [Sig.pairwise_iff, Sig.forall_mem_disjoint_iff, Sig.forall_mem_bound_iff]
             $[$loc]?
           try sig_reduce [] $[$loc]?)
      -- The tables' regions first, before `List.map` and `forall_eq` take them apart.
      sig_simp [$ls,*] [Sig.map_const_cons, List.map_nil, Sig.forall_mem_const_cons] $[$loc]?
      -- `List.filter` by `List.filter_cons`, a rewrite, rather than by its equations,
      -- which `simp` applies by unfolding: the kernel then checks the unfolding
      -- against the filtered list by evaluation, and evaluates the symbolic
      -- values in it (the stack arguments' memory reads) as far as it can:
      -- seconds for some contracts.
      sig_simp [$ls,*] [Sig.bufs, List.append_nil, List.map_nil, Sig.conj, Curry.apply, Curry.const, Curry.apply_const, ArgWord.ofRaw, Elem.size, List.filter_cons, List.filter_nil, List.map, List.cons_append, List.nil_append, List.all_cons, List.all_nil, Bool.not_true, Bool.not_false, Bool.and_true, Bool.and_false, Bool.true_and, Bool.false_and, Bool.and_self, Bool.or_true, Bool.true_or, Bool.or_false, Bool.false_or, Bool.false_eq_true, decide_true, decide_false, Nat.mul_one, Nat.one_mul, List.pairwise_cons, List.forall_mem_cons, List.not_mem_nil, List.Pairwise.nil, List.mem_cons, List.mem_nil_iff, forall_eq_or_imp, forall_eq, forall_false, implies_true, true_implies, false_implies, and_true, true_and, and_self, or_self, or_true, true_or, false_or, or_false, and_assoc, Nat.add_zero, List.zip_cons_cons, List.zip_nil_left, List.zip_nil_right, List.sum_cons, List.sum_nil, BitVec.setWidth_eq, BitVec.setWidth_32_64_32, BitVec.toNat_setWidth_32_64, BitVec.setWidth_setWidth_of_le, Nat.reduceAdd, Nat.reduceSub, Nat.reduceMul, Nat.reduceDiv,
        Nat.reduceLeDiff, Nat.reduceEqDiff, ↓reduceIte] $[$loc]?))

/-- Evaluates the precondition of a contract built with `Sig.contract` into
a conjunction of plain facts (see the module documentation). -/
syntax "sig_pre " "[" Lean.Parser.Tactic.simpLemma,* "]" (Lean.Parser.Tactic.location)? : tactic
macro_rules
  | `(tactic| sig_pre [$ls,*] $[$loc]?) => `(tactic| sig_eval [$ls,*] $[$loc]?)

/-- Evaluates the postcondition of a contract built with `Sig.contract`. -/
syntax "sig_post " "[" Lean.Parser.Tactic.simpLemma,* "]" (Lean.Parser.Tactic.location)? : tactic
macro_rules
  | `(tactic| sig_post [$ls,*] $[$loc]?) => `(tactic| sig_eval [$ls,*] $[$loc]?)

end VG
