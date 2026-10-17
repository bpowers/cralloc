module

public import Asmalloc.TCB.Print
public import Asmalloc.TCB.Sig

/-!
# Targets, contracts and verified artifacts

**Trusted.** This file defines what it *means* for an emitted function to be
verified. Every function the generated Rust contains is an `Artifact`, and an
`Artifact` cannot be constructed without a proof of `Verified`.

`Verified T c k` says, for the machine code `c` on target `T` and contract `k`:

1. **Total correctness and memory safety.** From every state satisfying
   `k.pre`, `c` terminates without faulting (so every memory access stays in
   the regions the state permits) in a state related to the initial one by
   `k.post` *and* by the target's calling-convention obligations
   `T.abiPreserved` (callee-saved registers, stack pointer, return address).
2. **Restartable sequences.** Wherever the operating system may abort a
   restartable sequence of `c`, the sequence is armed and the contract's
   `abort` holds (`RseqOk`).
3. **Non-vacuity.** Some state satisfies `k.pre`, and for each way `c` can
   branch (`Code.outcomes`: each `ite` both ways, and each exit and the
   commit of a restartable sequence) some state satisfying `k.pre` takes it
   (`Visits`), so no part of the code is verified only vacuously. The
   artifact gives these states as data (`Artifact.witnesses`), so each is a
   concrete state. This guards against an accidentally contradictory or
   narrow precondition; it does not replace reviewing the contract.

What a reviewer must read for each artifact is therefore: the ISA model and
ABI of its target (`TCB/<arch>/`), its contract (in `Spec/`), and its Rust
signature (`Sig`) and documentation (its `Api` in `Spec/`, and what its
registration file adds for the target). The code and the proof need not be
read.
-/

@[expose] public section

namespace VG

/-- A code-generation target: an ISA model, its printer, and its calling convention. -/
structure Target where
  /-- Short name; also the name of the generated Rust module (e.g. `x86_64`). -/
  name : String
  isa : ISA
  printer : Printer isa
  /-- Calling-convention obligations every function must meet when it
  returns, relating the entry state to the exit state (callee-saved
  registers and the stack pointer restored, return address intact). -/
  abiPreserved : isa.State → isa.State → Prop
  /-- The Rust `cfg` predicate under which this target's functions are
  compiled. It must exclude every configuration of the architecture that the
  model does not describe: memory is little-endian in every ISA model
  (`TCB/Mem.lean`), and pointers have `abi`'s width (`Abi.ptrBits`), which
  the contracts assume. -/
  rustCfg : String
  /-- The Rust ABI string of the generated functions (e.g. `sysv64`). -/
  rustAbi : String
  /-- The calling convention that `rustAbi` stands for. -/
  abi : Abi isa
  /-- Whether the instruction `i`, run in the state `a`, points the thread's
  restartable-sequence field (`rseq_cs`) at the routine's descriptor, as the
  last instruction of a sequence's arm must (`RseqOk`); never, on a target
  without restartable sequences. -/
  arms : isa.Instr → isa.State → Prop := fun _ _ => False
  /-- Whether the thread's restartable-sequence field points at the routine's
  descriptor in a state, as it must wherever the sequence may be aborted. -/
  armed : isa.State → Prop := fun _ => False
  /-- What an abort keeps: `rely p p'` when the thread, aborted in the state
  `p`, may resume at the abort handler in the state `p'`. The memory other
  threads may change meanwhile is the contract's (`Contract.env`). -/
  rely : isa.State → isa.State → Prop := fun _ _ => False

/-- The specification of one function. -/
structure Contract (M : ISA) where
  /-- Precondition on the entry state: argument registers, permitted memory
  regions, disjointness, CPU features, … -/
  pre : M.State → Prop
  /-- Postcondition relating the entry state to the exit state. -/
  post : M.State → M.State → Prop
  /-- What must hold, of the entry state and a state in which a restartable
  sequence may be aborted (`Code.abortStates`): typically, that nothing any
  other code can see has changed since entry. -/
  abort : M.State → M.State → Prop := fun _ _ => True
  /-- What other threads, and the operating system, may change while a
  restartable sequence is aborted: `env p p'` relates the state the sequence
  was aborted in to one the thread may resume in (with `Target.rely`).
  Typically, memory outside the structures other threads share is unchanged,
  and the invariant they keep still holds. -/
  env : M.State → M.State → Prop := fun _ _ => True

/-- The contract of a function with signature `sig` under the calling
convention `A`: the obligations the signature implies (see `TCB/Sig.lean`),
the further precondition `pre` and the postcondition `post`, both stated on
the arguments by name. If `writeArgs`, the function may also overwrite its
arguments passed in memory, where the calling convention gives them to the
callee; otherwise it may only read them. `stack` is the number of bytes of
stack below the stack pointer that the function's calls and frames use
(return addresses, arguments and saved registers), which no buffer
overlaps. A calling convention that does not model the arguments (`A.args`
is `none`) gives an unsatisfiable precondition, which `Verified` rejects. -/
def Sig.contract {M : ISA} (A : Abi M) (sig : Sig)
    (pre : Curry (sig.words A.ptrBits) (Mem → Prop) := Curry.const (fun _ => True) _)
    (post : sig.Post A.ptrBits) (writeArgs : Bool := false) (stack : Nat := 0) : Contract M :=
  let ws := sig.words A.ptrBits
  let widths := ws.map (·.bits A.ptrBits)
  { pre s := match A.args widths with
      | none => False
      | some vals =>
        let bufs := Sig.bufs sig.params (vals s)
        let all := bufs ++ (A.argArea widths s).map fun (r, w) => (r, w && writeArgs)
        A.wf widths stack s ∧
        A.rd s = (all.filter (!·.2)).map (·.1) ∧ A.wr s = (all.filter (·.2)).map (·.1) ∧
        all.Pairwise (fun a b => (a.2 || b.2) → a.1.Disjoint b.1) ∧
        (∀ r ∈ A.reserved stack s, ∀ a ∈ all, r.Disjoint a.1) ∧
        (∀ a ∈ bufs, a.1.base.toNat + a.1.len ≤ 2 ^ A.ptrBits) ∧
        Curry.apply ws pre (vals s) (A.mem s)
    post s s' := match A.args widths with
      | none => False
      | some vals => Curry.apply ws post (vals s) (A.mem s) (A.mem s')
          ((A.ret s').setWidth _) }

/-! ## Documenting the obligations `Sig.contract` implies

The emitter adds to each function's `# Safety` section what `Sig.contract`
requires of the memory each buffer is valid for (`Sig.validDoc`) and of where
its buffers are (`Sig.layoutDoc`), and a note if it may overwrite its
arguments (`Sig.layoutNote`), from the same signature, calling convention,
`writeArgs` and `stack` as its contract (`Artifact.ofSig`). The
calling convention names its argument area and reserved memory
(`Abi.argAreaDoc`, `Abi.reservedDoc`).
-/

/-- The buffers of the parameters `ps` as the documentation names them, and
whether each is writable: those of `Sig.bufs`, in the same order. -/
def Sig.bufNames : List (String × Param) → List (String × Bool)
  | [] => []
  | (_, .int ..) :: ps => Sig.bufNames ps
  | (n, .array w ..) :: ps => (s!"`{n}`", w) :: Sig.bufNames ps
  | (n, .slice w ..) :: ps => (s!"`{n}`", w) :: Sig.bufNames ps

/-- The size in bytes of the buffer a parameter stands for, as the
documentation states it: that of `Sig.bufs`, a number for an array and
`` `len` `` or `` `k * len` `` for a slice of `len` elements of `k` bytes;
`none` for an integer. -/
def Param.sizeDoc : Param → Option String
  | .int .. => none
  | .array _ e n => some s!"{n * e.size}"
  | .slice _ e len => some (if e.size = 1 then s!"`{len}`" else s!"`{e.size} * {len}`")

/-- The `# Safety` items stating the memory each buffer of `sig` must be
valid for, in order: the region `Sig.contract` lets the function read
(every buffer; loads may read the writable regions too) and write (the
writable buffers), of the size `Sig.bufs` gives it. -/
def Sig.validDoc (sig : Sig) : List String :=
  sig.params.filterMap fun (n, p) =>
    let access := match p with
      | .array true .. | .slice true .. => "reads and writes"
      | _ => "reads"
    p.sizeDoc.map fun size => s!"`{n}` must be valid for {access} of {size} bytes."

/-- `xs` as an English list joined by `conj`: "a", "a or b", "a, b or c". -/
def englishList (conj : String) : List String → String
  | [] => ""
  | [x] => x
  | [x, y] => s!"{x} {conj} {y}"
  | x :: xs => s!"{x}, " ++ englishList conj xs

/-- The `# Safety` items stating what the contracts `sig.contract A _ _
writeArgs stack` require of where the buffers are: that a writable buffer
overlaps no other buffer, nor the arguments in memory (`A.argArea`); that no
buffer overlaps the arguments in memory if the function may write them, nor
the memory `A.reserved stack` (the return address, and the stack that the
function's calls and frames use); and that no buffer wraps around the end of
the address space. -/
def Sig.layoutDoc {M : ISA} (A : Abi M) (sig : Sig) (writeArgs : Bool) (stack : Nat) :
    List String :=
  let bufs := Sig.bufNames sig.params
  let all := bufs.map (·.1)
  let w := (bufs.filter (·.2)).map (·.1)
  let r := (bufs.filter (!·.2)).map (·.1)
  let args := A.argAreaDoc ((sig.words A.ptrBits).map (·.bits A.ptrBits))
  let argsWritable := match args with
    | some (_, wr) => wr && writeArgs
    | none => false
  let argsName := (args.map (·.1)).toList
  let wObjs := (if w.length ≥ 2 then ["each other"] else []) ++ r ++
    (if argsWritable then [] else argsName)
  let allObjs := (if argsWritable then argsName else []) ++ (A.reservedDoc stack).toList
  let tail := ", ".intercalate (allObjs.map ("overlap " ++ ·)) ++
    (if allObjs.isEmpty then "" else ", or ") ++ "wrap around the end of the address space"
  (if w.isEmpty || wObjs.isEmpty then [] else
    [s!"{englishList "and" w} must not overlap {englishList "or" wObjs} (distinct Rust \
      objects never do)."]) ++
  match all with
    | [] => []
    | [b] => [s!"{b} must not {tail} (no Rust object does)."]
    | [a, b] => [s!"Neither {a} nor {b} may {tail} (no Rust object does)."]
    | _ => [s!"None of {englishList "and" all} may {tail} (no Rust object does)."]

/-- A paragraph saying that the function may overwrite its arguments in
memory, if the contracts `sig.contract A _ _ writeArgs _` let it. -/
def Sig.layoutNote {M : ISA} (A : Abi M) (sig : Sig) (writeArgs : Bool) : List String :=
  match A.argAreaDoc ((sig.words A.ptrBits).map (·.bits A.ptrBits)) with
  | some (d, true) => if writeArgs then
      [s!"The function may overwrite {d}, as the calling convention lets it."] else []
  | _ => []

/-- The states before each step of a restartable sequence's body, run from
`s` and not aborted, while it is inside the sequence: from before its first
step to before its commit, its last, and not after an exit is taken (which
leaves the sequence) or after a step faults. These are the states in which
the operating system may abort the sequence. -/
def bodyPrefixes (M : ISA) : List (Step M.Instr M.Cond) → M.State → List M.State
  | [], _ => []
  | .instr i :: rest, s => s :: (match M.exec i s with
    | some s₁ => bodyPrefixes M rest s₁
    | none => [])
  | .exit c _ _ :: rest, s => s :: (match M.eval c s with
    | some false => bodyPrefixes M rest s
    | _ => [])

/-- The states, of a run of `c` from `s`, in which a restartable sequence may
be aborted: for `rseq arm body`, those of its body after `arm`
(`bodyPrefixes`). The emitter allows a sequence only as the whole of a
function's code (`Rust.checkRseq`), so there are no others. -/
def Code.abortStates (M : ISA) : Prog M → M.State → List M.State
  | .rseq arm body, s => match execBlock M arm s with
    | some s₁ => bodyPrefixes M body s₁
    | none => []
  | _, _ => []

/-- What a restartable sequence `c` must satisfy, run from `s`: the last
instruction of its arm points the thread's `rseq_cs` at its descriptor
(`Target.arms`), whatever it pointed at before, so no sequence another one
started (in a signal handler, say) can be armed when this one starts; and in
every state `p` in which it may be aborted (`Code.abortStates`), `rseq_cs`
still points at it (`Target.armed`), the contract's `abort` holds, and every
state the thread may resume in after the abort (`Target.rely`,
`Contract.env`) satisfies the precondition. The abort handler jumps back to
the arm, the function's first instruction (`Rust.checkRseq`), so that is a
run of the function from the new state: a retry, which the first conjunct
of `Verified` covers (`Restarts`, `Verified.restarts`). True of code without
a sequence. -/
def RseqOk (T : Target) (c : Prog T.isa) (k : Contract T.isa) (s : T.isa.State) : Prop :=
  match c with
  | .rseq arm _ =>
    (∃ i a, arm.getLast? = some i ∧ execBlock T.isa arm.dropLast s = some a ∧ T.arms i a) ∧
    ∀ p ∈ c.abortStates T.isa s, T.armed p ∧ k.abort s p ∧
      ∀ p', T.rely p p' → k.env p p' → k.pre p'
  | _ => True

/-! ## Branch sites

So that no part of a function is verified only vacuously, each way its code
can branch must be taken by some state satisfying the precondition. The
*sites* of `c` are its `ite`s, and for a restartable sequence each of its
exits and its commit, numbered in the order they are written (a called
function's are its own). `Code.outcomes` lists what must be witnessed: each
`ite` both ways, and each exit and commit of a sequence taken. -/

/-- Whether a step of a sequence's body is an exit. -/
def Step.isExit {I C : Type} : Step I C → Bool
  | .exit .. => true
  | .instr _ => false

/-- The number of branch sites of `c`. -/
def Code.sites {I C : Type} : Code I C → Nat
  | .block _ => 0
  | .seq a b => a.sites + b.sites
  | .ite _ t e => 1 + t.sites + e.sites
  | .loop b _ => b.sites
  | .call .. => 0
  | .frame _ b _ => b.sites
  | .rseq _ body => body.countP Step.isExit + 1

/-- The site, the way taken, that each state satisfying the precondition
must reach for some witness. -/
def Code.outcomes {I C : Type} : Code I C → List (Nat × Bool)
  | .block _ => []
  | .seq a b => a.outcomes ++ b.outcomes.map fun (i, d) => (a.sites + i, d)
  | .ite _ t e => [(0, true), (0, false)] ++ t.outcomes.map (fun (i, d) => (1 + i, d)) ++
      e.outcomes.map fun (i, d) => (1 + t.sites + i, d)
  | .loop b _ => b.outcomes
  | .call .. => []
  | .frame _ b _ => b.outcomes
  | .rseq _ body => (List.range (body.countP Step.isExit + 1)).map (·, true)

/-- Where the body of a restartable sequence ends, run from `s` and not
aborted: the index of the exit it takes, or its length if it reaches its
commit and completes it; `none` if a step faults or reads an undefined
flag. -/
def bodyEnd (M : ISA) : List (Step M.Instr M.Cond) → M.State → Option Nat
  | [], _ => some 0
  | .instr i :: rest, s => match M.exec i s with
    | some s₁ => (bodyEnd M rest s₁).map (· + 1)
    | none => none
  | .exit c _ _ :: rest, s => match M.eval c s with
    | some false => (bodyEnd M rest s).map (· + 1)
    | some true => some 0
    | none => none

/-- A run of `c` from `s` takes the site `i` the way `d`: an `ite` with its
condition `d`; a sequence's exit or commit (`d` true), the site of the exit
it takes or of its commit (`bodyEnd`: the exits before it, `Code.sites`). -/
inductive Visits (M : ISA) : Prog M → M.State → Nat → Bool → Prop
  | seqL {c₁ c₂ s i d} : Visits M c₁ s i d → Visits M (.seq c₁ c₂) s i d
  | seqR {c₁ c₂ s s₁ i d} : Exec M c₁ s s₁ → Visits M c₂ s₁ i d →
      Visits M (.seq c₁ c₂) s (c₁.sites + i) d
  | ite {c t e s d} : M.eval c s = some d → Visits M (.ite c t e) s 0 d
  | iteT {c t e s i d} : M.eval c s = some true → Visits M t s i d →
      Visits M (.ite c t e) s (1 + i) d
  | iteF {c t e s i d} : M.eval c s = some false → Visits M e s i d →
      Visits M (.ite c t e) s (1 + t.sites + i) d
  | loopBody {b c s i d} : Visits M b s i d → Visits M (.loop b c) s i d
  | loopNext {b c s s₁ i d} : Exec M b s s₁ → M.eval c s₁ = some true →
      Visits M (.loop b c) s₁ i d → Visits M (.loop b c) s i d
  | frame {p b q s s₁ i d} : M.push p s = some s₁ → Visits M b s₁ i d →
      Visits M (.frame p b q) s i d
  | rseq {arm body s s₁ j} : execBlock M arm s = some s₁ → bodyEnd M body s₁ = some j →
      Visits M (.rseq arm body) s ((body.take j).countP Step.isExit) true

/-- The proof obligation for emitting `c` on target `T` with contract `k`. -/
def Verified (T : Target) (c : Prog T.isa) (k : Contract T.isa) : Prop :=
  (∀ s, k.pre s → ∃ s', Exec T.isa c s s' ∧ T.abiPreserved s s' ∧ k.post s s') ∧
  (∀ s, k.pre s → RseqOk T c k s) ∧
  (∃ s, k.pre s) ∧ (∀ o ∈ c.outcomes, ∃ s, k.pre s ∧ Visits T.isa c s o.1 o.2)

/-- The states a run of the routine `c` from `s` may start its last attempt
in, aborted any finite number of times: `s`, or a state the thread may
resume in after an abort (`Target.rely`, `Contract.env`) of an attempt
started in one of these. This, with the run from that state that is not
aborted (`Exec`), is what a routine does; `Verified.restarts`
(`Proof/Framework/Rseq.lean`) shows that every such run meets the contract.
That the thread is not aborted forever, so that some attempt completes, is
assumed (the scheduler is fair), not proved. -/
inductive Restarts (T : Target) (c : Prog T.isa) (k : Contract T.isa) :
    T.isa.State → T.isa.State → Prop
  | refl (s : T.isa.State) : Restarts T c k s s
  | abort {s p p' s' : T.isa.State} : p ∈ c.abortStates T.isa s → T.rely p p' → k.env p p' →
      Restarts T c k p' s' → Restarts T c k s s'

/-- A function's contract on every target: the contract under the calling
convention `A`, for an implementation whose calls and frames use `stack`
bytes of stack below the stack pointer (`Sig.contract`'s `stack`; a contract
that does not depend on it is the one with `stack` 0). -/
abbrev Contracts : Type 1 := {M : ISA} → Abi M → Nat → Contract M

/-- An operand of a function's assembly that its code names (`Piece.ref`),
other than its tables of constants and the functions it calls, which the
emitter declares itself:

* `sym path`: the Rust `static` or function at `path`, a `sym` operand. The
  model gives its address by the operand's name (on x86-64, `State.syms`).
* `const expr`: the Rust constant expression `expr`, a `const` operand,
  which may use the function's const generic parameters. The model gives its
  value by the operand's name (on x86-64, `State.params`), and the contract
  states what it may be. -/
inductive Operand
  | sym (path : String)
  | const (expr : String)
  deriving DecidableEq, Repr

/-- What a function is on every target: its Rust module, name and signature,
whether its contract lets it overwrite its arguments in memory
(`Sig.contract`'s `writeArgs`), its contract on every target (`contracts`),
and its documentation. A registration file makes an `Artifact` of it on each
target (`Api.artifact`, `TCB/Registration.lean`), adding any notes on the
implementation; the emitter adds the obligations the signature implies
(`Sig.validDoc`, and `Sig.layoutDoc`, which depends on the target).

The `Artifact` takes `contracts` from the `Api` with its name and signature,
and must be proven against `contracts` on its target (`Artifact.ofApi`): so
the Rust name, signature and documentation that `Spec/` gives a function are
those of the contract `Spec/` gives it, and nothing outside `Spec/` pairs
them. -/
structure Api where
  module : String
  name : String
  sig : Sig
  writeArgs : Bool := false
  /-- The function's contract on every target, `fun A stack => fooContract A
  stack`. The emitter refuses an artifact without one (`Rust.checkApi`). -/
  contracts : Option Contracts := none
  /-- The documentation, up to its `# Safety` section. -/
  summary : String
  /-- The items of the `# Safety` section, but for those `Sig.validDoc` and
  `Sig.layoutDoc` give (what memory each buffer must be valid for, and where
  it may be). -/
  safety : List String
  /-- The operands its code names, by name (`Operand`). -/
  operands : List (String × Operand) := []

/-- The documentation of `api` on a target: its summary, then the paragraphs
`notes`, then its `# Safety` section. -/
def Api.doc (api : Api) (notes : List String := []) : String :=
  api.summary ++ String.join (notes.map ("\n\n" ++ ·)) ++ "\n\n# Safety\n\n" ++
    "\n".intercalate (api.safety.map ("* " ++ ·))

/-- A verified function, ready to be emitted into the Rust crate. -/
structure Artifact where
  target : Target
  /-- The Rust module the function is emitted into, `<target>/<module>.rs`
  (e.g. `selftest`). -/
  module : String
  /-- The Rust function name. Must be unique within the target. -/
  name : String
  /-- The Rust signature, rendered as the parameter list and return type
  (`Sig.rust`). -/
  sig : Sig
  /-- Documentation for the generated Rust function. With what the emitter
  adds to it (`Sig.layoutNote`, `Sig.validDoc` and `Sig.layoutDoc`, which
  need it to end with its `# Safety` section if they add anything, and not
  to state what they do), it must state every requirement of `contract.pre`
  that the caller is responsible for. -/
  doc : String
  /-- Tables of 64-bit constants the code reads, by name: the emitter writes
  each as a `static` of that name, from which the code forms the table's
  address (`Abi.sym`; on x86-64, `leaSym`), and checks that every artifact of
  the target with a table of that name gives the same words
  (`Rust.checkConsts`). So `contract` is one for the calling convention
  `target.abi.withConsts consts` (`Abi.withConsts`). -/
  consts : List (String × List (BitVec 64)) := []
  /-- The other operands its code names (`Operand`), from its `Api`. -/
  operands : List (String × Operand) := []
  code : Prog target.isa
  contract : Contract target.isa
  verified : Verified target code contract
  /-- `Sig.contract`'s `writeArgs` and `stack` for `contract` (see `ofSig`). -/
  writeArgs : Bool := false
  stack : Nat := 0
  /-- `contract` is one that `Sig.contract` derives from `sig` for the
  target's calling convention, `writeArgs` and `stack`: so it reads the
  arguments and writes the return value where the calling convention places
  them for `sig`, and requires what `Sig.layoutDoc` documents of where the
  buffers are. -/
  ofSig : ∃ pre post,
      contract = sig.contract (target.abi.withConsts consts) pre post writeArgs stack := by
    exact ⟨_, _, rfl⟩
  /-- The contract of the function on every target, which an artifact made
  from an `Api` takes from it with its `name` and `sig` (`Api.contracts`).
  The emitter refuses an artifact without one (`Rust.checkApi`). -/
  contracts : Option Contracts := none
  /-- `contract` is `contracts` on the target, for `stack`: an artifact made
  from an `Api` is proven against the contract `Spec/` gives the function,
  not one chosen where the artifact is built. -/
  ofApi : contracts.elim True fun f => contract = f (target.abi.withConsts consts) stack := by
    first | exact True.intro | exact rfl
  /-- The code changes the stack pointer only by calls and returns and by
  the pushes and pops of frames, which are nested: no other instruction of
  it, or of the functions it calls, writes it. So a call instruction or a
  push only ever stores below the stack pointer on entry, where no Rust
  object lies. -/
  spSafe : code.all (fun i => !target.isa.writesSp i) = true := by decide +kernel
  /-- The CPU features beyond the target's baseline ISA that the code needs,
  by their Rust `target_feature` names: exactly those its instructions, and
  those of the functions it calls, require (`ISA.requires`). The emitter
  checks this (`Rust.checkFeatures`) and adds their presence to the
  function's `# Safety` section, which `doc` must end with when this is not
  empty. -/
  features : List String := []
  /-- States satisfying the precondition, one taking each way the code can
  branch (`Code.outcomes`): data, so each is a concrete state the emitter
  can evaluate. -/
  witnesses : List target.isa.State
  /-- Some witness satisfies the precondition, and each outcome has one that
  takes it. -/
  witnessOk : (∃ w ∈ witnesses, contract.pre w) ∧
    ∀ o ∈ code.outcomes, ∃ w ∈ witnesses, contract.pre w ∧ Visits target.isa code w o.1 o.2

end VG
