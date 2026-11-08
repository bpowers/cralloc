module

public import Cralloc.TCB.Mem

/-!
# Structured assembly programs and their semantics

**Trusted.** Every architecture model instantiates `ISA`: a machine state, a
type of straight-line instructions with a partial semantics, and a type of
branch conditions.

Programs are `Code`: straight-line blocks of real instructions glued together
with structured control flow (`ite` and do-while `loop`), calls of other
functions (`call`) and stack frames (`frame`). The printer
(`TCB/Print.lean`) lowers the structured control flow to labels and
conditional jumps, a call to a call instruction naming the function, which
is emitted separately, and a frame to its push, its body and its pop.

## Calls

`call name body` is a call of the function `name`, whose code is `body`: its
semantics is that of the call instruction (`ISA.call`), then `body`, then the
function's return instruction (`ISA.ret`). The emitter checks that the
function it calls by that name is exactly `body` (`VG.Rust.files`).

A call instruction stores a return address, which is where the code of the
caller is loaded: the model does not know it. So the machine state supplies
the values the model does not know, for all the calls to come, and a
contract never constrains them, so that a proof covers every value.
The return instruction faults in the model unless it returns to that
address. The stack pointer is only changed by calls and returns and by
frames (`ISA.writesSp`, `Artifact.spSafe`), which are nested, so that the
memory a call instruction or a push stores to is below the stack pointer on
entry: never memory that the caller of the function uses.

## Stack frames

`frame push body pop` saves registers on the stack around `body`, or passes
arguments on the stack to the functions `body` calls: its semantics is that
of the push instruction (`ISA.push`), then `body`, then the pop instruction
(`ISA.pop`). The push moves the stack pointer down, stores registers there
and makes those bytes a new region at the head of the writable regions `wr`
(so that `body`, and the functions it calls, may read and write them). The
pop faults unless the stack pointer and the writable regions are those the
push left, and the frame has the size popped; it loads one register from the
frame, moves the stack pointer back up and removes the region. So frames are
nested, the stack pointer is back where it was after each one, and code may
access a frame only while it lies at or above the stack pointer (memory
below the stack pointer may be overwritten at any time, e.g. by a signal
handler). Push and pop instructions fault outside a frame (`ISA.exec`).

## Restartable sequences

`rseq arm body` is a Linux restartable sequence. `arm` runs first and
points the thread's `rseq_cs` field at the sequence's descriptor (which
`Verified` checks: `Target.arms`); `body` is the sequence itself, from its
start to just past its last step, the commit. If the thread is preempted,
migrated or signalled with its instruction pointer inside `body`, the
operating system aborts the sequence: the thread resumes at the abort
handler, which jumps back to `arm`, with the registers it was interrupted
with. What runs before `arm` does not run again (`Artifact.setup`). `body` may
leave early by an exit (`Step.exit`): to code after the sequence, or to an
external target, such as a slow path that is a function of its own (`ExecExit`).

`Exec` describes the run that is not aborted, which the operating system
guarantees nothing else interferes with. Where an abort may happen, before
each step of `body`, is `Code.abortStates` (`TCB/Artifact.lean`); what
`Verified` requires there, and how a run with any number of aborts follows,
is in `TCB/Artifact.lean` and `Proof/Framework/Rseq.lean`.

## Semantics and safety

A `jump` never completes: its only run is the one that reaches the jump
(`ExecExit.jump`).

`Exec M c s s'` means: running `c` from `s` terminates in `s'` without
faulting. An instruction faults (`exec` returns `none`) on any memory access
outside the regions the state permits, and on reading an undefined flag; so
a proof of `∃ s', Exec M c s s' ∧ …` establishes termination, memory safety
and functional correctness at once.
-/

@[expose] public section

namespace VG

/-- One access to memory: `bytes` bytes at `addr`, read or written
(`write`), and the value read or written (little-endian, as a number). -/
structure Event where
  addr : Addr
  bytes : Nat
  write : Bool
  value : Nat
  deriving DecidableEq, Repr

/-- The interface between an architecture model and the generic framework. -/
structure ISA where
  State : Type
  Instr : Type
  Cond : Type
  /-- Semantics of a straight-line instruction; `none` means the machine faults. -/
  exec : Instr → State → Option State
  /-- The memory accesses of an instruction run from the first state to the
  second, in order (`Event`): of the instructions a restartable sequence may
  contain, whose events its contract may constrain (`Contract.trace`). -/
  events : Instr → State → State → List Event
  /-- Evaluate a branch condition; `none` if it depends on an undefined flag. -/
  eval : Cond → State → Option Bool
  /-- Semantics of a call instruction, up to the transfer of control to the
  called function: storing the return address and moving the stack pointer.
  The values the model does not know are the next of the state's. `none` if
  it faults. -/
  call : State → Option State
  /-- Semantics of the return instruction of a called function, in the state
  `s₂`, given the state `s₁` in which the function was entered (just after
  the call instruction): `none` unless it returns to the return address that
  the call instruction stored. -/
  ret : State → State → Option State
  /-- Whether an instruction may write the stack pointer (other than as the
  push or pop of a frame). -/
  writesSp : Instr → Bool
  /-- Semantics of the push instruction of a frame: moving the stack pointer
  down, storing registers there, and adding those bytes as a region at the
  head of the writable regions. `none` if it faults (or is not a push). -/
  push : Instr → State → Option State
  /-- Semantics of the pop instruction of a frame, in the state `s₂`, given
  the state `s₁` just after the push: `none` unless the stack pointer and
  the writable regions are those the push left, and the frame (the region at
  their head) has the size popped; otherwise loading a register from the
  frame, moving the stack pointer back up and removing the region. -/
  pop : Instr → State → State → Option State
  /-- The CPU features beyond the target's baseline ISA that an instruction
  needs, by their Rust `target_feature` names (e.g. `bmi2`), as the vendor
  manual lists them for it (the Intel SDM's "CPUID Feature Flag"). On a CPU
  without them the instruction is undefined (#UD), which `exec` does not
  model: `Artifact.features` makes their presence the caller's obligation
  instead. -/
  requires : Instr → List String

/-- A step of the body of a restartable sequence (`Code.rseq`). -/
inductive Step (I C : Type) where
  | instr (i : I)
  /-- If `c` holds, leave the sequence: run the instructions `cold` (laid
  out of line), then continue after the sequence (`ext = none`), or jump to
  the external target `ext` (`some t`), code outside the routine that the
  model does not run. -/
  | exit (c : C) (cold : List I) (ext : Option String)

/-- The instructions of a step, those it runs out of line included. -/
def Step.instrs {I C : Type} : Step I C → List I
  | .instr i => [i]
  | .exit _ cold _ => cold

/-- Structured code. -/
inductive Code (I C : Type) where
  | block (is : List I)
  | seq (c₁ c₂ : Code I C)
  /-- `if c then t else e` -/
  | ite (c : C) (t e : Code I C)
  /-- `do body while c` -/
  | loop (body : Code I C) (c : C)
  /-- A call of the function `name`, whose code is `body`. -/
  | call (name : String) (body : Code I C)
  /-- `push; body; pop`: a stack frame around `body`. -/
  | frame (push : I) (body : Code I C) (pop : I)
  /-- A restartable sequence (Linux `rseq`): `arm` tells the operating
  system which sequence the thread is in, and `body` is the sequence, whose
  last step commits. The operating system may abort the sequence before any
  step of `body`; it then resumes the thread at `arm`. Its semantics
  (`Exec`) is the run that is not aborted; `Verified` adds what must hold
  wherever it may be (`Code.abortStates`). -/
  | rseq (arm : List I) (body : List (Step I C))
  /-- Run the straight-line instructions `is`, then jump to the external
  target `t`: a tail call of code outside the routine, which the model does
  not run, with the registers as `is` leaves them. The emitter allows one
  only right after a restartable sequence, `.seq (.rseq arm body) (.jump is
  t)` (`Rust.checkRseq`): the code that runs after the commit, outside the
  sequence, where it cannot be aborted. -/
  | jump (is : List I) (t : String)

/-- `if c then t else e`, with `t` laid out after the function's return
(cold): `Code.ite c (.seq (.seq (.block []) (.block [])) t) e`. It is an
`ite`, whose `then` branch runs two empty blocks (which do nothing) before
`t`, so its semantics are `ite`'s by construction; the two empty blocks are
only the mark by which the printer (`Printer.lower`) places `t` out of line,
so that the code around the `ite` keeps its addresses whatever `t`'s size. -/
def Code.iteCold {I C : Type} (c : C) (t e : Code I C) : Code I C :=
  .ite c (.seq (.seq (.block []) (.block [])) t) e

/-- Whether every instruction of `c`, and of the functions it calls, satisfies `p`. -/
def Code.all {I C : Type} (p : I → Bool) : Code I C → Bool
  | .block is => is.all p
  | .seq a b => a.all p && b.all p
  | .ite _ t e => t.all p && e.all p
  | .loop b _ => b.all p
  | .call _ b => b.all p
  | .frame i b j => p i && b.all p && p j
  | .rseq arm body => arm.all p && body.all fun st => st.instrs.all p
  | .jump is _ => is.all p

/-- The features that the instructions of `c`, and of the functions it
calls, require (`ISA.requires`), with repetitions. -/
def Code.requires {I C : Type} (req : I → List String) : Code I C → List String
  | .block is => is.flatMap req
  | .seq a b => a.requires req ++ b.requires req
  | .ite _ t e => t.requires req ++ e.requires req
  | .loop b _ => b.requires req
  | .call _ b => b.requires req
  | .frame i b j => req i ++ b.requires req ++ req j
  | .rseq arm body => arm.flatMap req ++ body.flatMap fun st => st.instrs.flatMap req
  | .jump is _ => is.flatMap req

/-- The calls in `c`, and in the functions it calls: each function's name
and code. -/
def Code.calls {I C : Type} : Code I C → List (String × Code I C)
  | .block _ => []
  | .seq a b => a.calls ++ b.calls
  | .ite _ t e => t.calls ++ e.calls
  | .loop b _ => b.calls
  | .call n b => (n, b) :: b.calls
  | .frame _ b _ => b.calls
  | .rseq .. | .jump .. => []

variable (M : ISA)

abbrev Prog := Code M.Instr M.Cond

/-- Run a straight-line block. -/
def execBlock : List M.Instr → M.State → Option M.State
  | [], s => some s
  | i :: is, s =>
    match M.exec i s with
    | none => none
    | some s₁ => execBlock is s₁

/-- How the body of a restartable sequence ends: after its last step, or by
an exit that continues after the sequence (`done`), or by an exit to the
external target `t` (`exit`). -/
inductive Out (S : Type) where
  | done (s : S)
  | exit (t : String) (s : S)

/-- Run the body of a restartable sequence, not aborted; `none` if it
faults. -/
def runBody : List (Step M.Instr M.Cond) → M.State → Option (Out M.State)
  | [], s => some (.done s)
  | .instr i :: rest, s =>
    match M.exec i s with
    | none => none
    | some s₁ => runBody rest s₁
  | .exit c cold ext :: rest, s =>
    match M.eval c s with
    | none => none
    | some false => runBody rest s
    | some true =>
      match execBlock M cold s with
      | none => none
      | some s₁ => some (match ext with | none => .done s₁ | some t => .exit t s₁)

/-- Big-step semantics: terminating, non-faulting executions. -/
inductive Exec : Prog M → M.State → M.State → Prop
  | block {is s s'} : execBlock M is s = some s' → Exec (.block is) s s'
  | seq {c₁ c₂ s₁ s₂ s₃} : Exec c₁ s₁ s₂ → Exec c₂ s₂ s₃ → Exec (.seq c₁ c₂) s₁ s₃
  | iteT {c th el s s'} : M.eval c s = some true → Exec th s s' → Exec (.ite c th el) s s'
  | iteF {c th el s s'} : M.eval c s = some false → Exec el s s' → Exec (.ite c th el) s s'
  | loopExit {body c s s'} :
      Exec body s s' → M.eval c s' = some false → Exec (.loop body c) s s'
  | loopNext {body c s s' s''} :
      Exec body s s' → M.eval c s' = some true → Exec (.loop body c) s' s'' →
      Exec (.loop body c) s s''
  | call {name body s s₁ s₂ s'} :
      M.call s = some s₁ → Exec body s₁ s₂ → M.ret s₁ s₂ = some s' → Exec (.call name body) s s'
  | frame {i j body s s₁ s₂ s'} :
      M.push i s = some s₁ → Exec body s₁ s₂ → M.pop j s₁ s₂ = some s' →
      Exec (.frame i body j) s s'
  | rseq {arm body s s₁ s'} :
      execBlock M arm s = some s₁ → runBody M body s₁ = some (.done s') →
      Exec (.rseq arm body) s s'

/-- Executions that end by jumping to an external target (`Step.exit` with
`some t`): running `c` from `s` reaches the jump to `t` in `s'` without
faulting. An exit inside a call or a frame has no such execution (it would
leave the stack as the call or frame left it), so code with one cannot be
verified. -/
inductive ExecExit : Prog M → M.State → String → M.State → Prop
  | seqL {c₁ c₂ s t s'} : ExecExit c₁ s t s' → ExecExit (.seq c₁ c₂) s t s'
  | seqR {c₁ c₂ s s₁ t s'} : Exec M c₁ s s₁ → ExecExit c₂ s₁ t s' → ExecExit (.seq c₁ c₂) s t s'
  | iteT {c th el s t s'} : M.eval c s = some true → ExecExit th s t s' → ExecExit (.ite c th el) s t s'
  | iteF {c th el s t s'} : M.eval c s = some false → ExecExit el s t s' → ExecExit (.ite c th el) s t s'
  | loopBody {body c s t s'} : ExecExit body s t s' → ExecExit (.loop body c) s t s'
  | loopNext {body c s s₁ t s'} :
      Exec M body s s₁ → M.eval c s₁ = some true → ExecExit (.loop body c) s₁ t s' →
      ExecExit (.loop body c) s t s'
  | rseq {arm body s s₁ t s'} :
      execBlock M arm s = some s₁ → runBody M body s₁ = some (.exit t s') →
      ExecExit (.rseq arm body) s t s'
  | jump {is t s s'} : execBlock M is s = some s' → ExecExit (.jump is t) s t s'

end VG
