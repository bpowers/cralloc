module

public import Asmalloc.TCB.Code

/-!
# Rust signatures and calling conventions

**Trusted.** A `Sig` describes the signature of a generated function in Rust
terms: integers passed by value, and pointers standing for references to
arrays (`&[T; N]`, `&mut [T; N]`), slices (`&[T]`, `&mut [T]`, passed as
a pointer and a length) and lists of slices (`&[&[T]]`, passed as a pointer
to their addresses and lengths, and their number). An `Abi` describes a target's calling convention:
where each argument is (registers, register pairs, stack slots) and what the
caller's frame looks like to the callee.

From the two, `Sig.contract` (in `TCB/Artifact.lean`) derives the part of a
contract that the Rust types determine, so that contracts do not spell it out
by hand for each target:

* where each argument is;
* the memory the function may read (every buffer, the slices each list of
  slices lists, as its descriptors say on entry, and arguments passed in
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
  registers; no Rust object lies below the stack pointer);
* that the pointers, the slice lengths, what the descriptors of a list of
  slices say (where its slices are) and the stack pointer are public (an
  argument only in the bits of its width: see `Sig.contract`).

The generated Rust functions take raw pointers (`Sig.rust`), so these are
obligations on the caller, which a caller passing references (as the crate's
safe wrappers do) meets by construction. A contract adds only what the types
do not say: any further precondition (e.g. a bound on a length), the
postcondition, and which integer arguments are public.
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
  /-- An integer; `pub` says whether it is public for constant-time purposes. -/
  | int (ty : IntTy) (pub : Bool)
  /-- A `*const [T; n]` standing for a `&[T; n]`, or a `*mut [T; n]` standing
  for a `&mut [T; n]` if `writable`. -/
  | array (writable : Bool) (elem : Elem) (n : Nat)
  /-- A `*const T` standing for a `&[T]`, or a `*mut T` standing for a
  `&mut [T]` if `writable`, followed by a `usize` parameter named `len`: the
  length of the slice, in elements. -/
  | slice (writable : Bool) (elem : Elem) (len : String)
  /-- A `*const [usize; 2]` standing for a `&[&[T]]`, followed by a `usize`
  parameter named `count`: `count` descriptors, each the address of a slice
  of `T` and its length in elements, as two pointer-sized words (in that
  order). The descriptors and the slices they list are read-only; where the
  slices are (their addresses and lengths) is public, as a slice's address
  and length are, and their contents are not. -/
  | slices (elem : Elem) (count : String)
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
  | .int ty _ => [.int (ty.bits ptrBits)]
  | .array .. => [.addr]
  | .slice .. => [.addr, .int ptrBits]
  | .slices .. => [.addr, .int ptrBits]

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
  and AArch64 give the whole register, whose upper bits the convention
  leaves unspecified); `none` if the convention passes them in a way that
  is not modelled. -/
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
  /-- The part of the state other than the arguments that is public (the
  stack pointer). -/
  pub : M.State → M.State → Prop
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

/-! ## Tables of constants

An artifact may read tables of constants (`Artifact.consts`): the emitter
writes each as a Rust `static` of its name, and the code forms its address
from the name (on AArch64, `adrSym`: `adrp` and `add`; on ARMv7, `adrSym`:
`movw`, `movt` and an `add` of `pc`; on x86-64, `leaSym`: `lea` of a
RIP-relative operand), which the linker resolves to
the static (see `TCB/Rust.lean`). `A.withConsts cs` is the
calling convention as a contract for such code sees it: the state gives each
table's address (`A.sym`), which is public, and the memory holds the table
there; the code may read it (the tables come last in the regions it may
read, after those the contract gives the arguments), no writable region or
reserved memory overlaps it, and it does not wrap around the address space:
as for a Rust `static`, a distinct immutable object holding exactly those
words. With no tables, or on a target whose code cannot name a static, it is
`A`. -/

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
          (∀ r ∈ A.wr s, t.Disjoint r) ∧ ∀ r ∈ A.reserved n s, t.Disjoint r)
      pub s₁ s₂ := A.pub s₁ s₂ ∧ ∀ c ∈ cs, f s₁ c.1 = f s₂ c.1 }

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
  | (_, .slices ..) :: ps, _ :: _ :: vs => Sig.bufs ps vs
  | _, _ => []

/-- The buffers of the arrays among the parameters, given the arguments'
values: for each, its parameter's name and the region `Sig.bufs` gives it. -/
def Sig.arrays : List (String × Param) → List (BitVec 64) → List (String × Region)
  | [], _ => []
  | (_, .int ..) :: ps, _ :: vs => Sig.arrays ps vs
  | (n, .array _ e k) :: ps, p :: vs => (n, ⟨p, k * e.size⟩) :: Sig.arrays ps vs
  | (_, .slice ..) :: ps, _ :: _ :: vs => Sig.arrays ps vs
  | (_, .slices ..) :: ps, _ :: _ :: vs => Sig.arrays ps vs
  | _, _ => []

/-- For each `(name, n)` of `written`, the function changes no byte at
offset `n` or beyond of the buffer of the array parameter `name`, between
the entry state `s` and the exit state `s'`: it writes at most its first `n`
bytes. With `written` empty, this says nothing. -/
def Sig.writtenWithin {M : ISA} (A : Abi M) (sig : Sig) :
    List (String × Nat) → M.State → M.State → Prop
  | [], _, _ => True
  | written, s, s' =>
    match A.args ((sig.words A.ptrBits).map (·.bits A.ptrBits)) with
    | none => True
    | some vals => ∀ w ∈ written, ∀ b ∈ Sig.arrays sig.params (vals s), b.1 = w.1 →
        ∀ i, w.2 ≤ i → i < b.2.len →
          A.mem s' (b.2.base + BitVec.ofNat 64 i) = A.mem s (b.2.base + BitVec.ofNat 64 i)

/-- The `n` descriptors of a list of slices at `p` (`Param.slices`), on a
target with `ptrBits`-bit pointers: each `2 * (ptrBits / 8)` bytes. -/
def Sig.descRegion (ptrBits : Nat) (p : Addr) (n : Nat) : Region := ⟨p, n * (2 * (ptrBits / 8))⟩

/-- The slices of elements of type `e` that the `n` descriptors at `p` list
in the memory `m`: for each, the address in its first word (zero-extended,
as `Abi.args` gives a pointer narrower than 64 bits) and the length in its
second, in elements. -/
def Sig.listed (ptrBits : Nat) (m : Mem) (e : Elem) (p : Addr) (n : Nat) : List Region :=
  (List.range n).map fun i =>
    let d := p + BitVec.ofNat 64 (i * (2 * (ptrBits / 8)))
    ⟨(m.readW d ptrBits).setWidth 64, (m.readW (d + BitVec.ofNat 64 (ptrBits / 8)) ptrBits).toNat * e.size⟩

/-- The read-only memory of the lists of slices among the parameters, given
the arguments' values and the memory on entry: for each list, its
descriptors, then the slices they list (`Sig.listed`). -/
def Sig.lists (ptrBits : Nat) (m : Mem) : List (String × Param) → List (BitVec 64) → List Region
  | [], _ => []
  | (_, .int ..) :: ps, _ :: vs => Sig.lists ptrBits m ps vs
  | (_, .array ..) :: ps, _ :: vs => Sig.lists ptrBits m ps vs
  | (_, .slice ..) :: ps, _ :: _ :: vs => Sig.lists ptrBits m ps vs
  | (_, .slices e _) :: ps, p :: n :: vs =>
    Sig.descRegion ptrBits p n.toNat :: Sig.listed ptrBits m e p n.toNat ++ Sig.lists ptrBits m ps vs
  | _, _ => []

/-- The descriptors of the lists of slices among the parameters, given the
arguments' values: what they say (where the slices are) is public. -/
def Sig.descs (ptrBits : Nat) : List (String × Param) → List (BitVec 64) → List Region
  | [], _ => []
  | (_, .int ..) :: ps, _ :: vs => Sig.descs ptrBits ps vs
  | (_, .array ..) :: ps, _ :: vs => Sig.descs ptrBits ps vs
  | (_, .slice ..) :: ps, _ :: _ :: vs => Sig.descs ptrBits ps vs
  | (_, .slices ..) :: ps, p :: n :: vs => Sig.descRegion ptrBits p n.toNat :: Sig.descs ptrBits ps vs
  | _, _ => []

/-- Whether each machine-level argument is public: pointers and lengths are. -/
def Param.pubs : Param → List Bool
  | .int _ pub => [pub]
  | .array .. => [true]
  | .slice .. => [true, true]
  | .slices .. => [true, true]

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
  | .int ty _ => s!"{name}: {ty.rust}"
  | .array w e n => s!"{name}: *{if w then "mut" else "const"} [{e.rust}; {n}]"
  | .slice w e len => s!"{name}: *{if w then "mut" else "const"} {e.rust}, {len}: usize"
  | .slices _ count => s!"{name}: *const [usize; 2], {count}: usize"

/-- The Rust parameter list and return type. -/
def Sig.rust (sig : Sig) : String :=
  "(" ++ ", ".intercalate (sig.params.map fun p => p.2.rust p.1) ++ ")" ++
    match sig.ret with
    | none => ""
    | some ty => s!" -> {ty.rust}"

end VG
