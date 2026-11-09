module

public import Cralloc.TCB.Code

/-!
# Rust signatures and calling conventions

**Trusted.** A `Sig` describes the signature of a generated function in Rust
terms: integers passed by value, and pointers standing for references to
arrays (`&[T; N]`, `&mut [T; N]`) and slices (`&[T]`, `&mut [T]`, passed as
a pointer and a length). An `Abi` describes a target's calling convention:
where each argument is (registers, stack slots) and what the caller's frame
looks like to the callee.

From the two, `Sig.contract` (in `TCB/Artifact.lean`) derives the part of a
contract that the Rust types determine, so that contracts do not spell it out
by hand for each target:

* where each argument is;
* the memory the function may read (every buffer, and arguments passed in
  memory) and write (every `mut` buffer, and, if the contract asks for it,
  arguments passed in memory that the convention gives to the callee);
* that a `mut` buffer overlaps no other buffer and no argument passed in
  memory, and that nothing overlaps the return address (a `&mut` is unique,
  and no Rust object contains the callee's argument area or return-address
  slot);
* that no buffer wraps around the end of the address space (no Rust
  allocation does);
* that no buffer overlaps the stack below the stack pointer that the
  function's calls and frames use (for return addresses, arguments and saved
  registers; no Rust object lies below the stack pointer).

The generated Rust functions take raw pointers (`Sig.rust`), so these are
obligations on the caller, which a caller passing references meets by
construction. A contract adds only what the types do not say: any further
precondition (e.g. a bound on a length) and the postcondition.
-/

@[expose] public section

namespace VG

/-- Element types of arrays and slices. -/
inductive Elem
  | u8 | u32 | u64
  /-- `[e; n]` -/
  | array (e : Elem) (n : Nat)
  deriving DecidableEq, Repr

/-- The size in bytes. -/
def Elem.size : Elem → Nat
  | .u8 => 1
  | .u32 => 4
  | .u64 => 8
  | .array e n => n * e.size

def Elem.rust : Elem → String
  | .u8 => "u8"
  | .u32 => "u32"
  | .u64 => "u64"
  | .array e n => s!"[{e.rust}; {n}]"

/-- Integer types passed by value. -/
inductive IntTy | u32 | u64 | usize
  deriving DecidableEq, Repr

/-- The width in bits, on a target with `ptrBits`-bit pointers. -/
def IntTy.bits (ptrBits : Nat) : IntTy → Nat | .u32 => 32 | .u64 => 64 | .usize => ptrBits
def IntTy.rust : IntTy → String | .u32 => "u32" | .u64 => "u64" | .usize => "usize"

/-- A parameter of a generated function. -/
inductive Param
  /-- An integer. -/
  | int (ty : IntTy)
  /-- A `*const [T; n]` standing for a `&[T; n]`, or a `*mut [T; n]` standing
  for a `&mut [T; n]` if `writable`. -/
  | array (writable : Bool) (elem : Elem) (n : Nat)
  /-- A `*const T` standing for a `&[T]`, or a `*mut T` standing for a
  `&mut [T]` if `writable`, followed by a `usize` parameter named `len`: the
  length of the slice, in elements. -/
  | slice (writable : Bool) (elem : Elem) (len : String)
  deriving DecidableEq, Repr

structure Sig where
  params : List (String × Param)
  ret : Option IntTy := none
  deriving DecidableEq, Repr

/-- One machine-level argument: a pointer, or an integer of the given width. -/
inductive ArgWord | addr | int (bits : Nat)
  deriving DecidableEq, Repr

def ArgWord.bits (ptrBits : Nat) : ArgWord → Nat | .addr => ptrBits | .int n => n

/-- What a contract sees of an argument: a pointer as an address, an integer
as a bit vector of its width. -/
abbrev ArgWord.Ty : ArgWord → Type | .addr => Addr | .int n => BitVec n

/-- The value of an argument from the 64 bits `Abi.args` gives for it: an
integer narrower than 64 bits is their low bits, whatever the others are. -/
def ArgWord.ofRaw : (w : ArgWord) → BitVec 64 → w.Ty | .addr, v => v | .int n, v => v.setWidth n

/-- The machine-level arguments a parameter is passed as. -/
def Param.words (ptrBits : Nat) : Param → List ArgWord
  | .int ty => [.int (ty.bits ptrBits)]
  | .array .. => [.addr]
  | .slice .. => [.addr, .int ptrBits]

def Sig.words (sig : Sig) (ptrBits : Nat) : List ArgWord :=
  sig.params.flatMap fun p => p.2.words ptrBits

/-- `Curry ws α` is `w₁.Ty → … → wₙ.Ty → α`: contracts receive the arguments
by name, as the parameters of a function. -/
abbrev Curry : List ArgWord → Type → Type
  | [], α => α
  | w :: ws, α => w.Ty → Curry ws α

def Curry.apply {α : Type} : (ws : List ArgWord) → Curry ws α → List (BitVec 64) → α
  | [], f, _ => f
  | w :: ws, f, v :: vs => Curry.apply ws (f (w.ofRaw v)) vs
  | w :: ws, f, [] => Curry.apply ws (f (w.ofRaw 0)) []

def Curry.const {α : Type} (a : α) : (ws : List ArgWord) → Curry ws α
  | [] => a
  | _ :: ws => fun _ => Curry.const a ws

/-- A calling convention: how the arguments of a function are passed, and
what the caller's frame looks like to it. -/
structure Abi (M : ISA) where
  /-- Pointer width. -/
  ptrBits : Nat
  /-- For arguments of the given widths (in bits), in order: their values on
  entry, as 64 bits, of which an argument narrower than 64 bits is the low
  bits (`ArgWord.ofRaw`); the other bits are not necessarily zero (x86-64
  gives the whole register, whose upper bits the convention leaves
  unspecified); `none` if the convention passes them in a way that is not
  modelled. -/
  args : List Nat → Option (M.State → List (BitVec 64))
  /-- The memory holding the arguments passed in memory (if any), and whether
  the convention lets the callee write it (which a contract may decline). -/
  argArea : List Nat → M.State → List (Region × Bool)
  /-- Memory that no buffer or argument area overlaps, for a function whose
  calls and frames use `n` bytes of stack: the return address, and the `n`
  bytes below the stack pointer (`stackBelow`). -/
  reserved : (n : Nat) → M.State → List Region
  /-- Facts about the entry state that hold for every call of a function
  whose calls and frames use `n` bytes of stack (e.g. the part of the stack
  the function sees does not wrap around). -/
  wf : List Nat → (n : Nat) → M.State → Prop
  /-- The memory of a state, and the regions it permits reading and writing. -/
  mem : M.State → Mem
  rd : M.State → List Region
  wr : M.State → List Region
  /-- The register(s) an integer result is returned in, as 64 bits: a
  narrower result is in the low bits. -/
  ret : M.State → BitVec 64
  /-- How documentation names `argArea ws`, for arguments of the widths
  `ws`: `none` if it is empty on every state; otherwise its name and
  whether it is writable (the flag `argArea` gives each of its regions). -/
  argAreaDoc : List Nat → Option (String × Bool)
  /-- How documentation names `reserved n`: `none` if it is empty on every
  state. -/
  reservedDoc : Nat → Option String
  /-- The address of each `static` the code can name, by name
  (`Artifact.consts`), on a target whose code can name one; `none` on the
  others. -/
  sym : Option (M.State → String → BitVec 64) := none
  /-- The thread pointer, on a target that has one (x86-64: the FS base). -/
  tp : Option (M.State → Addr) := none
  /-- The value of each compile-time constant the code can name
  (`Operand.const`), by name, on a target whose code can name one. -/
  param : Option (M.State → String → BitVec 64) := none
  /-- Whether a routine finds the thread's rseq area once, in its setup
  (`Artifact.setup`), so that the memory accesses of its attempts do not include
  reading `__rseq_offset` (`Contract.trace`). -/
  setupFindsArea : Bool := false
  /-- Whether the code may form the address of the slabs from their page alone
  (AArch64's `adrp` with no `:lo12:`), so that the slabs must start on a page. -/
  slabsOnPage : Bool := false
  /-- How many bytes of a class's 8-byte mask entry a C routine reads (`Spec.CEntry`), of
  which it tests the low 32 bits: where a load scaled by 8 reads all of it (AArch64's
  `ldr x, [b, i, lsl #3]`; a 32-bit load scales by 4), all of them. -/
  maskBytes : Nat := 4

/-! ## Tables of constants

An artifact may read tables of constants (`Artifact.consts`): the emitter
writes each as a Rust `static` of its name, and the code forms its address
from the name (on x86-64, `leaSym`: `lea` of a RIP-relative operand), which
the linker resolves to the static (see `TCB/Rust.lean`). `A.withConsts cs` is
the calling convention as a contract for such code sees it: the state gives
each table's address (`A.sym`), and the memory holds the table there; the
code may read it (the tables come last in the regions it may read, after
those the contract gives the arguments), no writable region or reserved
memory overlaps it, and it does not wrap around the address space: as for a
Rust `static`, a distinct immutable object holding exactly those words. With
no tables, or on a target whose code cannot name a static, it is `A`. -/

/-- The tables `cs` at the addresses `f` gives their names, each `8 * length`
bytes. -/
def Abi.constRegions (f : String → BitVec 64) (cs : List (String × List (BitVec 64))) :
    List Region :=
  cs.map fun c => ⟨f c.1, 8 * c.2.length⟩

/-- The memory `m` holds each table at its address: word `i` (64 bits,
little-endian) at the address plus `8 i`. -/
def Abi.constsHeld (m : Mem) (f : String → BitVec 64) (cs : List (String × List (BitVec 64))) :
    Prop :=
  ∀ c ∈ cs, ∀ i < c.2.length, m.readW (f c.1 + BitVec.ofNat 64 (8 * i)) 64 = c.2.getD i 0

def Abi.withConsts {M : ISA} (A : Abi M) (cs : List (String × List (BitVec 64))) : Abi M :=
  match A.sym, cs with
  | none, _ | _, [] => A
  | some f, cs =>
    { A with
      rd s := (A.rd s).take ((A.rd s).length - cs.length)
      wf ws n s := A.wf ws n s ∧
        (A.rd s).drop ((A.rd s).length - cs.length) = Abi.constRegions (f s) cs ∧
        Abi.constsHeld (A.mem s) (f s) cs ∧
        (∀ t ∈ Abi.constRegions (f s) cs, t.base.toNat + t.len ≤ 2 ^ A.ptrBits ∧
          (∀ r ∈ A.wr s, t.Disjoint r) ∧ ∀ r ∈ A.reserved n s, t.Disjoint r) }

/-! ## Memory beyond the buffers

A function may be granted memory its signature does not describe: a
routine's thread-local area, a `static` it writes, a block the state names.
`A.withExtra (some e)` is the calling convention as a contract for such code
sees it: `e s` lists the extra regions of the state `s` and whether each is
writable, as a function of what the state holds (its symbols, thread
pointer, constants and memory); the code may read each and write the
writable ones (they come after the buffers in the regions it may read and
write, before any tables of constants), and they do not wrap around the
address space, overlap each other where either is writable, overlap the
stack the function uses, or overlap a buffer where either is writable.
`Sig.contract` then pins the state's regions to exactly the buffers and
these, as it does to the buffers alone. -/

def Abi.withExtra {M : ISA} (A : Abi M) : Option (M.State → List (Region × Bool)) → Abi M
  | none => A
  | some e =>
    let ro s := ((e s).filter (!·.2)).map (·.1)
    let rw s := ((e s).filter (·.2)).map (·.1)
    { A with
      rd s := (A.rd s).take ((A.rd s).length - (ro s).length)
      wr s := (A.wr s).take ((A.wr s).length - (rw s).length)
      wf ws n s := A.wf ws n s ∧
        (A.rd s).drop ((A.rd s).length - (ro s).length) = ro s ∧
        (A.wr s).drop ((A.wr s).length - (rw s).length) = rw s ∧
        (∀ x ∈ e s, x.1.base.toNat + x.1.len ≤ 2 ^ A.ptrBits) ∧
        (e s).Pairwise (fun a b => (a.2 || b.2) → a.1.Disjoint b.1) ∧
        (∀ x ∈ e s, ∀ r ∈ A.reserved n s, x.1.Disjoint r) ∧
        (∀ x ∈ e s, ∀ r ∈ (A.rd s).take ((A.rd s).length - (ro s).length), x.2 → x.1.Disjoint r) ∧
        (∀ x ∈ e s, ∀ r ∈ (A.wr s).take ((A.wr s).length - (rw s).length), x.1.Disjoint r) }

/-- The `n` bytes below the stack pointer `sp`, if any. -/
def stackBelow (sp : Addr) : Nat → List Region
  | 0 => []
  | n + 1 => [⟨sp - BitVec.ofNat 64 (n + 1), n + 1⟩]

/-- The buffers the parameters refer to, given the arguments' values, and
whether each is writable. -/
def Sig.bufs : List (String × Param) → List (BitVec 64) → List (Region × Bool)
  | [], _ => []
  | (_, .int ..) :: ps, _ :: vs => Sig.bufs ps vs
  | (_, .array m e n) :: ps, p :: vs => (⟨p, n * e.size⟩, m) :: Sig.bufs ps vs
  | (_, .slice m e _) :: ps, p :: l :: vs => (⟨p, l.toNat * e.size⟩, m) :: Sig.bufs ps vs
  | _, _ => []

/-- The width of the return value (0 if there is none). -/
def Sig.retBits (sig : Sig) (ptrBits : Nat) : Nat := match sig.ret with
  | none => 0
  | some ty => ty.bits ptrBits

/-- The type of a postcondition: on the arguments, the memory on entry and
exit and the return value (of width 0 if there is none). -/
abbrev Sig.Post (sig : Sig) (ptrBits : Nat) : Type :=
  Curry (sig.words ptrBits) (Mem → Mem → BitVec (sig.retBits ptrBits) → Prop)

/-! ## Rendering -/

def Param.rust (name : String) : Param → String
  | .int ty => s!"{name}: {ty.rust}"
  | .array w e n => s!"{name}: *{if w then "mut" else "const"} [{e.rust}; {n}]"
  | .slice w e len => s!"{name}: *{if w then "mut" else "const"} {e.rust}, {len}: usize"

/-- The Rust parameter list and return type. -/
def Sig.rust (sig : Sig) : String :=
  "(" ++ ", ".intercalate (sig.params.map fun p => p.2.rust p.1) ++ ")" ++
    match sig.ret with
    | none => ""
    | some ty => s!" -> {ty.rust}"

end VG
