module

public import Cralloc.Proof.Framework.Body

/-!
# A stretch of a restartable sequence's body, as one step

The semantics folds over a body four ways (`Body.lean`), and a routine's proof wants all four of
every way out. So a stretch of steps is proved once, for what follows it whatever that is, and the
four folds of a whole body come from composing its stretches: `Through` for steps that take no
exit, and `Leaves` for steps that end in one.
-/

@[expose] public section

namespace VG

variable {M : ISA} {i : M.Instr} {g : M.Cond} {cold : List M.Instr} {ext : Option String}
  {l l₁ l₂ : List (Step M.Instr M.Cond)} {a b d : M.State} {ps ps₁ ps₂ : List M.State}
  {es es₁ es₂ e : List Event} {o : Out M.State} {j : Nat}

/-- The steps `l`, run from `a`, take no exit and reach `b`, passing through the states `ps`
(`bodyPrefixes`), with the memory accesses `es`. -/
structure Through (M : ISA) (l : List (Step M.Instr M.Cond)) (a b : M.State) (ps : List M.State)
    (es : List Event) : Prop where
  run : ∀ rest, runBody M (l ++ rest) a = runBody M rest b
  pre : ∀ rest, bodyPrefixes M (l ++ rest) a = ps ++ bodyPrefixes M rest b
  stop : ∀ rest, bodyEnd M (l ++ rest) a = (bodyEnd M rest b).map (· + l.length)
  ev : ∀ rest, bodyEvents M (l ++ rest) a = es ++ bodyEvents M rest b

/-- The steps `l`, run from `a`, end in the exit at the index `j`, with the result `o`, passing
through the states `ps`, with the memory accesses `es`, whatever follows them. -/
structure Leaves (M : ISA) (l : List (Step M.Instr M.Cond)) (a : M.State) (o : Out M.State)
    (ps : List M.State) (es : List Event) (j : Nat) : Prop where
  run : ∀ rest, runBody M (l ++ rest) a = some o
  pre : ∀ rest, bodyPrefixes M (l ++ rest) a = ps
  stop : ∀ rest, bodyEnd M (l ++ rest) a = some j
  ev : ∀ rest, bodyEvents M (l ++ rest) a = es

namespace Through

theorem nil (a : M.State) : Through M [] a a [] [] :=
  ⟨fun _ => rfl, fun _ => rfl, fun rest => by simp, fun _ => rfl⟩

theorem instr (h : M.exec i a = some b) (he : M.events i a b = e) (t : Through M l b d ps es) :
    Through M (.instr i :: l) a d (a :: ps) (e ++ es) where
  run rest := by rw [List.cons_append, runBody_instr h]; exact t.run rest
  pre rest := by rw [List.cons_append, bodyPrefixes_instr h, t.pre rest]; rfl
  stop rest := by
    rw [List.cons_append, bodyEnd_instr h, t.stop rest, List.length_cons]
    cases bodyEnd M rest d <;> simp [Nat.add_assoc]
  ev rest := by rw [List.cons_append, bodyEvents_instr h, t.ev rest, he, List.append_assoc]

/-- An instruction that accesses no memory. -/
theorem reg (h : M.exec i a = some b) (he : M.events i a b = []) (t : Through M l b d ps es) :
    Through M (.instr i :: l) a d (a :: ps) es :=
  instr h he t

/-- An instruction with one access. -/
theorem mem {e₀ : Event} (h : M.exec i a = some b) (he : M.events i a b = [e₀])
    (t : Through M l b d ps es) : Through M (.instr i :: l) a d (a :: ps) (e₀ :: es) :=
  instr h he t

theorem skip (h : M.eval g a = some false) (t : Through M l a d ps es) :
    Through M (.exit g cold ext :: l) a d (a :: ps) es where
  run rest := by rw [List.cons_append, runBody_skip h]; exact t.run rest
  pre rest := by rw [List.cons_append, bodyPrefixes_skip h, t.pre rest]; rfl
  stop rest := by
    rw [List.cons_append, bodyEnd_skip h, t.stop rest, List.length_cons]
    cases bodyEnd M rest d <;> simp [Nat.add_assoc]
  ev rest := by rw [List.cons_append, bodyEvents_skip h, t.ev rest]

theorem trans (t₁ : Through M l₁ a b ps₁ es₁) (t₂ : Through M l₂ b d ps₂ es₂) :
    Through M (l₁ ++ l₂) a d (ps₁ ++ ps₂) (es₁ ++ es₂) where
  run rest := by rw [List.append_assoc, t₁.run, t₂.run]
  pre rest := by rw [List.append_assoc, t₁.pre, t₂.pre, List.append_assoc]
  stop rest := by
    rw [List.append_assoc, t₁.stop, t₂.stop, List.length_append]
    cases bodyEnd M rest d with
    | none => rfl
    | some v =>
      simp only [Option.map_some, Option.some.injEq]
      omega
  ev rest := by rw [List.append_assoc, t₁.ev, t₂.ev, List.append_assoc]

/-! The folds of a whole body that takes no exit. -/

theorem run0 (t : Through M l a b ps es) : runBody M l a = some (.done b) := by
  have h := t.run []
  rwa [List.append_nil] at h

theorem pre0 (t : Through M l a b ps es) : bodyPrefixes M l a = ps := by
  have h := t.pre []
  rwa [List.append_nil, show bodyPrefixes M [] b = [] from rfl, List.append_nil] at h

theorem stop0 (t : Through M l a b ps es) : bodyEnd M l a = some l.length := by
  have h := t.stop []
  rw [List.append_nil] at h
  rw [h]
  simp [bodyEnd_nil']

theorem ev0 (t : Through M l a b ps es) : bodyEvents M l a = es := by
  have h := t.ev []
  rwa [List.append_nil, bodyEvents_nil', List.append_nil] at h

end Through

namespace Leaves

/-- An exit taken, with no code of its own. -/
theorem take (h : M.eval g a = some true) :
    Leaves M (.exit g [] ext :: l) a (match ext with | none => .done a | some t => .exit t a) [a] [] 0 where
  run rest := by
    rw [List.cons_append, runBody_take h execBlock_nil']
    cases ext <;> rfl
  pre rest := by rw [List.cons_append, bodyPrefixes_take h]
  stop rest := by rw [List.cons_append, bodyEnd_take h]
  ev rest := by rw [List.cons_append, bodyEvents_take h, blockEvents_nil']

theorem instr (h : M.exec i a = some b) (he : M.events i a b = e) (t : Leaves M l b o ps es j) :
    Leaves M (.instr i :: l) a o (a :: ps) (e ++ es) (j + 1) where
  run rest := by rw [List.cons_append, runBody_instr h]; exact t.run rest
  pre rest := by rw [List.cons_append, bodyPrefixes_instr h, t.pre rest]
  stop rest := by rw [List.cons_append, bodyEnd_instr h, t.stop rest]; rfl
  ev rest := by rw [List.cons_append, bodyEvents_instr h, t.ev rest, he]

/-- An instruction that accesses no memory. -/
theorem reg (h : M.exec i a = some b) (he : M.events i a b = []) (t : Leaves M l b o ps es j) :
    Leaves M (.instr i :: l) a o (a :: ps) es (j + 1) :=
  instr h he t

/-- An instruction with one access. -/
theorem mem {e₀ : Event} (h : M.exec i a = some b) (he : M.events i a b = [e₀])
    (t : Leaves M l b o ps es j) : Leaves M (.instr i :: l) a o (a :: ps) (e₀ :: es) (j + 1) :=
  instr h he t

theorem skip (h : M.eval g a = some false) (t : Leaves M l a o ps es j) :
    Leaves M (.exit g cold ext :: l) a o (a :: ps) es (j + 1) where
  run rest := by rw [List.cons_append, runBody_skip h]; exact t.run rest
  pre rest := by rw [List.cons_append, bodyPrefixes_skip h, t.pre rest]
  stop rest := by rw [List.cons_append, bodyEnd_skip h, t.stop rest]; rfl
  ev rest := by rw [List.cons_append, bodyEvents_skip h, t.ev rest]

/-- After steps that take no exit. -/
theorem after (t : Through M l₁ a b ps₁ es₁) (x : Leaves M l₂ b o ps₂ es₂ j) :
    Leaves M (l₁ ++ l₂) a o (ps₁ ++ ps₂) (es₁ ++ es₂) (j + l₁.length) where
  run rest := by rw [List.append_assoc, t.run, x.run]
  pre rest := by rw [List.append_assoc, t.pre, x.pre]
  stop rest := by rw [List.append_assoc, t.stop, x.stop]; rfl
  ev rest := by rw [List.append_assoc, t.ev, x.ev]

/-- With steps after it, which it does not reach. -/
theorem before (x : Leaves M l₁ a o ps es j) (l₂ : List (Step M.Instr M.Cond)) :
    Leaves M (l₁ ++ l₂) a o ps es j where
  run rest := by rw [List.append_assoc]; exact x.run _
  pre rest := by rw [List.append_assoc]; exact x.pre _
  stop rest := by rw [List.append_assoc]; exact x.stop _
  ev rest := by rw [List.append_assoc]; exact x.ev _

/-! The folds of a whole body. -/

theorem run0 (x : Leaves M l a o ps es j) : runBody M l a = some o := by
  have h := x.run []
  rwa [List.append_nil] at h

theorem pre0 (x : Leaves M l a o ps es j) : bodyPrefixes M l a = ps := by
  have h := x.pre []
  rwa [List.append_nil] at h

theorem stop0 (x : Leaves M l a o ps es j) : bodyEnd M l a = some j := by
  have h := x.stop []
  rwa [List.append_nil] at h

theorem ev0 (x : Leaves M l a o ps es j) : bodyEvents M l a = es := by
  have h := x.ev []
  rwa [List.append_nil] at h

end Leaves

end VG
