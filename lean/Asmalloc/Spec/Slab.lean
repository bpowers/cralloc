module

public import Asmalloc.TCB.Artifact

/-!
# The front end: slabs, magazines and the routines that pop and push

**Trusted** (as every file in `Spec/`). The layout of the slabs, the slab
invariant, the abstract slab, and the contracts of the four routines that
work on them: pop and push, each inline (an `asm!` block whose class and
mask are constants) and out of line (a function taking the class in a
register and reading the mask from a table). See docs/design.md, "Front
end: magazines under rseq".

## Layout

`SLABS` is a static array of `SLAB_COUNT` slabs of `SLAB_BYTES` bytes, one
per CPU number. A slab's first words are the slab words, one per class: the
word of class `c` of CPU `i` is at `SLABS + i * SLAB_BYTES + 8 c`
(`wordAddr`). A magazine of `n` slots (`n` a power of two) is `8 n` bytes,
aligned to its size; slot 0 is a link word and slot `n - 1` is unused, and
the slots between hold pointers. A slab word is zero (no magazine), a
marker (`IsMarker`: its lowest bit set, a drainer's generation above every
slot-number bit), or the address of the next free slot of the magazine loaded for
that class: `8 s` past its base, for `1 ≤ s ≤ n - 1`, slots `1` to `s - 1`
holding the cached pointers (`Mag.ofWord`). The mask of an `n`-slot
magazine is `8 n - 16` (`magMask`): the bits of the slot number but its
lowest. The out-of-line routines read the mask of the class `c` from the
`u32` at `MAG_MASKS + 4 c`.

## The invariant

For every CPU below `SLAB_COUNT`, the slab word of the routine's class is
zero, a marker, or a slot address with `s ≥ 1` of a magazine of the
class's size (`WordOk`), and the magazine lies apart from the slabs, the
thread's rseq area, `__rseq_offset` and the stack the calling convention
reserves (`Ctx.Apart`). That nothing else refers to the magazine, and that
the pointers are distinct free objects, are properties of the whole
allocator (docs/design.md, "The invariant"), kept by the slow paths and the
drainer, which the routines do not rely on: they only move the boundary
between the free slots and the cached pointers. The abstraction is of one
word at a time (`Mag.ofWord`), so a slow path's compare-and-store of a whole
word is a change from one abstract magazine to another.

## The routines

Each routine reads the thread's rseq area (glibc's `__rseq_offset` from the
thread pointer, `rseq_cs` at 8, `cpu_id` at 4), points `rseq_cs` at its
descriptor, reads the CPU number, takes the slow path if it is not below
`SLAB_COUNT` (as it is not for a thread without a registration, whose
`cpu_id` reads as a negative number), reads the slab word, tests it with the mask, and then pops
the top pointer or pushes one, committing by storing the new slab word.
What each may touch is one function of the state (`Ctx.regions`): the
variable `__rseq_offset` and `cpu_id` (read), `rseq_cs` (written), the
mask's table entry (read, out of line), and, if the CPU number is in range,
the slab word and the magazine it names (written). Every marker fails both
mask tests, so no routine stores over one.

## What the operating system and other threads may do

A routine is a restartable sequence: it may be aborted, and other threads
may run, before its commit; then it starts again from its arm, for the CPU
number it then reads. Wherever it may be aborted it has kept the invariant
(the contract's `abort`), its arguments (`Sig.contract`'s `abort`), and the
stack pointer and the registers the calling convention or `asm!` has it
keep (`Target.abiPreserved`, `Artifact.clobbersOk`); the abort keeps its
registers (`Target.rely`). While it is aborted, other threads and the
drainer may change any memory but `__rseq_offset` and the thread's
reserved stack, keeping the invariant (`env`). The routine proves its
precondition from these alone when it starts again: the regions it may
access are recomputed from the memory then (`Ctx.regions`), and their
layout (`Ctx.Layout`) is one of addresses an abort does not change.
-/

@[expose] public section

namespace VG.Spec.Slab

/-! ## Layout -/

/-- `log2` of the bytes of a slab. -/
def SLAB_SHIFT : Nat := 12
/-- The bytes of a slab. -/
def SLAB_BYTES : Nat := 2 ^ SLAB_SHIFT
/-- The number of slabs: one for every CPU number the kernel may give a
thread on x86-64. A larger one (or a negative one, read as 32 bits) takes
the slow path. -/
def SLAB_COUNT : Nat := 8192
/-- The classes a slab has a word for: a class's word lies in its slab. -/
def CLASS_LIMIT : Nat := SLAB_BYTES / 8
/-- The magazine sizes are `2 ^ k` slots, `MAG_LOG_MIN ≤ k ≤ MAG_LOG_MAX`:
at least 4 (with 2, the mask is zero), and few enough that the mask, and
so the slot number, is in the low 32 bits a routine tests. -/
def MAG_LOG_MIN : Nat := 2
def MAG_LOG_MAX : Nat := 16
/-- A marker is a slab word whose lowest `MARKER_SHIFT` bits are `1`: its
lowest bit set, and the drainer's generation it carries above every bit of
the slot number of every allowed magazine size, so that both mask tests
fail on it. -/
def MARKER_SHIFT : Nat := MAG_LOG_MAX + 3
/-- Offsets of the fields of `struct rseq` (`include/uapi/linux/rseq.h`). -/
def RSEQ_CS : Nat := 8
def RSEQ_CPU_ID : Nat := 4

/-- The names the code gives the symbols it uses: glibc's offset of the
thread's rseq area (`Target.rseqOffsetSym` on x86-64), the slabs, the masks'
table and a routine's slow path. -/
def RSEQ_OFFSET : String := "__rseq_offset"
def SLABS : String := "SLABS"
def MAG_MASKS : String := "MAG_MASKS"
def SLOW : String := "slow"
/-- The names of the constants an inline routine uses: its class's byte
offset in a slab and its mask. -/
def COFF : String := "coff"
def MASK : String := "mask"
/-- The label an inline push exits to: the Rust function returns `false`. -/
def MISS : String := "miss"

/-- The Rust paths of the symbols. The crate defines `SLABS` (`SLAB_COUNT`
slabs of `SLAB_BYTES` bytes, zeroed, aligned to `SLAB_BYTES`, written only
by the routines and the drainer), `MAG_MASKS` (a `[u32; N]`, immutable, entry
`c` the mask of class `c`'s magazines for each class `c` used) and the slow
paths, and makes glibc's `__rseq_offset` visible as `front::__rseq_offset`. -/
def path (n : String) : String := "crate::front::" ++ n

/-- The mask of a magazine of `n` slots. -/
def magMask (n : Nat) : Nat := 8 * n - 16

/-- A mask of an allowed magazine size. -/
def MaskOk (mask : Nat) : Prop :=
  ∃ k, MAG_LOG_MIN ≤ k ∧ k ≤ MAG_LOG_MAX ∧ mask = magMask (2 ^ k)

/-- The slots of a magazine whose mask is `mask`. -/
def slotsOf (mask : Nat) : Nat := (mask + 16) / 8

/-- The address of the slab word of class `cls` of CPU `cpu`. -/
def wordAddr (slabs : Addr) (cpu cls : Nat) : Addr :=
  slabs + BitVec.ofNat 64 (cpu * SLAB_BYTES + 8 * cls)

/-! ## Slab words and the abstract magazine -/

/-- The slot a word names in a magazine of `n` slots. -/
def slot (n : Nat) (w : BitVec 64) : Nat := w.toNat % (8 * n) / 8

/-- The base of the magazine of `n` slots a word names. -/
def magBase (n : Nat) (w : BitVec 64) : Addr := w - BitVec.ofNat 64 (8 * slot n w)

/-- The word is a slot address, of a slot other than 0. -/
def IsSlot (n : Nat) (w : BitVec 64) : Prop := w.toNat % 8 = 0 ∧ 1 ≤ slot n w

instance (n : Nat) (w : BitVec 64) : Decidable (IsSlot n w) := by
  unfold IsSlot; infer_instance

/-- The word is a marker: a drainer has taken the magazine. -/
def IsMarker (w : BitVec 64) : Prop := w.toNat % 2 ^ MARKER_SHIFT = 1

instance (w : BitVec 64) : Decidable (IsMarker w) := by unfold IsMarker; infer_instance

/-- A slab word of a class whose magazines have `n` slots: no magazine, a
marker, or the next free slot of a magazine. -/
def WordOk (n : Nat) (w : BitVec 64) : Prop :=
  w.toNat = 0 ∨ IsMarker w ∨ IsSlot n w

/-- What a slab word stands for. -/
inductive Mag
  /-- No magazine (zero). -/
  | absent
  /-- A marker (any word with its lowest bit set): being drained. -/
  | marker
  /-- A magazine at `base`, whose cached pointers are `items`, the last on
  top. -/
  | stack (base : Addr) (items : List (BitVec 64))
  deriving DecidableEq, Repr

/-- The magazine the word `w` names in memory `m`, its magazines having `n`
slots: the pointers in slots `1` to `s - 1` for the slot `s` the word
names. (Of a word that is not `WordOk`, any.) -/
def Mag.ofWord (n : Nat) (m : Mem) (w : BitVec 64) : Mag :=
  if w.toNat = 0 then .absent else if w.toNat % 2 = 1 then .marker else
    .stack (magBase n w) ((List.range (slot n w - 1)).map fun i =>
      m.readW (magBase n w + BitVec.ofNat 64 (8 * (i + 1))) 64)

/-- Popping: the top pointer and the rest, of a magazine with one. -/
def Mag.pop : Mag → Option (BitVec 64 × Mag)
  | .stack b items => items.getLast?.map fun x => (x, .stack b items.dropLast)
  | _ => Option.none

/-- Pushing `x` on a magazine of `n` slots with a free slot to spare (the
last slot stays unused). -/
def Mag.push (n : Nat) (x : BitVec 64) : Mag → Option Mag
  | .stack b items => if items.length + 2 < n then some (.stack b (items ++ [x])) else Option.none
  | _ => Option.none

/-! ## Spare stacks

A slab's cold part holds the heads of stacks of spare magazines. A head is
zero (an empty stack), a marker, or the address of the top magazine, whose
link slot (its slot 0) holds the next head. A magazine on a spare stack is
reachable from nowhere else, so its link slots lie apart from everything a
routine writes (`StackOk`). -/

/-- The magazines of a stack whose head is `h`, in memory `m`, top first:
each a nonzero multiple of 8 whose link slot holds the next head, the last
next head zero. -/
inductive Chain (m : Mem) : BitVec 64 → List Addr → Prop
  | nil : Chain m 0 []
  | cons {h : BitVec 64} {l : List Addr} : h.toNat ≠ 0 → h.toNat % 8 = 0 →
      Chain m (m.readW h 64) l → Chain m h (h :: l)

/-- A stack head: a marker, or the head of a stack whose magazines' link
slots each satisfy `ok` (`Ctx.Apart`). -/
def StackOk (m : Mem) (h : BitVec 64) (ok : Region → Prop) : Prop :=
  IsMarker h ∨ ∃ l, Chain m h l ∧ ∀ x ∈ l, ok ⟨x, 8⟩

/-- What the slow path's routines return when the thread has no valid CPU
number: no slab word or stack head is ever 2 (it is neither zero, a marker
nor a multiple of 8). -/
def NO_CPU : Nat := 2

/-- Memory `m'` is `m` but in the regions `rs`. -/
def Unchanged (rs : List Region) (m m' : Mem) : Prop :=
  ∀ x, (∀ r ∈ rs, ¬ r.Contains x 1) → m' x = m x

/-- Memory `m'` is `m` in the regions `rs`. -/
def Kept (rs : List Region) (m m' : Mem) : Prop :=
  ∀ r ∈ rs, ∀ x, r.Contains x 1 → m' x = m x

/-! ## A routine's view of the state

Through the calling convention `A`, so that the contracts hold on every
target: the thread pointer, the symbols' addresses and the constants
(`Abi.tp`, `Abi.sym`, `Abi.param`; zero on a target without them, whose
contract the precondition then makes unsatisfiable). -/

/-- Which routine, and how it is given its word in a slab. -/
inductive Form
  /-- A pop or push of a pointer, given the class's offset in a slab
  (`COFF`) and the mask (`MASK`) as constants. -/
  | inline
  /-- A pop or push of a pointer, given the class as the first argument and
  the mask in `MAG_MASKS`. -/
  | table
  /-- The slow path's store-if-equal of the word at the byte offset in the
  first argument (`cls`). -/
  | cas
  /-- Push the magazine in the second argument on the stack whose head is
  the word at the byte offset in the first argument. -/
  | stackPush
  /-- Pop the top magazine off the stack whose head is the word at the byte
  offset in the first argument. -/
  | stackPop
  deriving DecidableEq, Repr

/-- A routine: its calling convention, form and signature. -/
structure Ctx (M : ISA) where
  A : Abi M
  form : Form
  sig : Sig

namespace Ctx

variable {M : ISA} (c : Ctx M) (s : M.State)

def mem : Mem := c.A.mem s
def tp : Addr := match c.A.tp with | some f => f s | none => 0
def sym (n : String) : Addr := match c.A.sym with | some f => f s n | none => 0
def param (n : String) : BitVec 64 := match c.A.param with | some f => f s n | none => 0

/-- The `i`-th argument (`Sig.contract`'s values). -/
def arg (i : Nat) : BitVec 64 :=
  match c.A.args ((c.sig.words c.A.ptrBits).map (·.bits c.A.ptrBits)) with
  | some f => (f s).getD i 0
  | none => 0

/-- The thread's rseq area, its `rseq_cs` and its `cpu_id`. -/
def area : Addr := c.tp s + (c.mem s).readW (c.sym s RSEQ_OFFSET) 64
def csField : Region := ⟨c.area s + BitVec.ofNat 64 RSEQ_CS, 8⟩
def cpuField : Region := ⟨c.area s + BitVec.ofNat 64 RSEQ_CPU_ID, 4⟩
def cpu : Nat := ((c.mem s).readW (c.cpuField s).base 32).toNat

/-- The routine's class: the index of its word in a slab. The slow path's
routines take a byte offset, of which they use the word it falls in, so
that any offset names a word of the slab. -/
def cls : Nat := match c.form with
  | .inline => (c.param s COFF).toNat / 8
  | .table => (c.arg s 0).toNat
  | .cas | .stackPush | .stackPop => (c.arg s 0).toNat % SLAB_BYTES / 8

/-- Where the out-of-line routine reads its class's mask. -/
def maskAddr : Addr := c.sym s MAG_MASKS + BitVec.ofNat 64 (4 * c.cls s)

/-- The mask, and the slots of the class's magazines. -/
def mask : Nat := match c.form with
  | .inline => (c.param s MASK).toNat
  | .table => ((c.mem s).readW (c.maskAddr s) 32).toNat
  | .cas | .stackPush | .stackPop => 0
def slots : Nat := slotsOf (c.mask s)

/-- The address of the slab word, and the word, of the class for the
thread's CPU. -/
def word : Addr := wordAddr (c.sym s SLABS) (c.cpu s) (c.cls s)
def w : BitVec 64 := (c.mem s).readW (c.word s) 64

/-- The CPU number has a slab. -/
def InRange : Prop := c.cpu s < SLAB_COUNT

instance : Decidable (c.InRange s) := by unfold InRange; infer_instance

/-- The abstract magazine of the class for the thread's CPU. -/
def mag : Mag := Mag.ofWord (c.slots s) (c.mem s) (c.w s)

/-- The magazine's region, if the word names one. -/
def magRegion : Region := ⟨magBase (c.slots s) (c.w s), 8 * c.slots s⟩

/-- The slabs, `__rseq_offset`, the mask's table entry, and the link slot
of the magazine a stack push pushes. -/
def slabs : Region := ⟨c.sym s SLABS, SLAB_COUNT * SLAB_BYTES⟩
def rsVar : Region := ⟨c.sym s RSEQ_OFFSET, 8⟩
def maskEntry : Region := ⟨c.maskAddr s, 4⟩
def link : Region := ⟨c.arg s 1, 8⟩

/-- What a routine may access whatever its CPU number, and whether it may
write it: `__rseq_offset` and `cpu_id` (read), `rseq_cs` (written), the
mask's table entry (`table`, read) and the pushed magazine's link slot
(`stackPush`, written). -/
def fixed : List (Region × Bool) :=
  [(c.rsVar s, false), (c.cpuField s, false), (c.csField s, true)] ++
  match c.form with
  | .table => [(c.maskEntry s, false)]
  | .stackPush => [(c.link s, true)]
  | _ => []

/-- What a routine may touch beyond its word, if its CPU number has a slab:
a pop's or push's magazine, if the word names one (written); a stack pop's
top magazine's link slot, if the head names one (read). -/
def more : List (Region × Bool) := match c.form with
  | .inline | .table => if IsSlot (c.slots s) (c.w s) then [(c.magRegion s, true)] else []
  | .cas | .stackPush => []
  | .stackPop => if (c.w s).toNat ≠ 0 ∧ (c.w s).toNat % 2 = 0 then [(⟨c.w s, 8⟩, false)] else []

/-- **The memory a routine may access**, and whether it may write it: what
`fixed` names, and, if the CPU number has a slab, the word and what `more`
names. A function of the state, so recomputed on a retry. -/
def regions : List (Region × Bool) :=
  c.fixed s ++ (if c.InRange s then (⟨c.word s, 8⟩, true) :: c.more s else [])

/-- Where a magazine, or a spare stack's link slot, may be: not wrapping
around the end of the address space, and apart from the slabs, from what
`fixed` names and from the stack the calling convention reserves. -/
def Apart (r : Region) : Prop :=
  r.base.toNat + r.len ≤ 2 ^ c.A.ptrBits ∧ r.Disjoint (c.slabs s) ∧
    (∀ x ∈ c.fixed s, r.Disjoint x.1) ∧ ∀ q ∈ c.A.reserved 0 s, r.Disjoint q

/-- Where the slabs and what `fixed` names are: none wraps around the end
of the address space; those of `fixed` are apart from the slabs, from the
reserved stack, and from each other where one is written; and the slabs are
apart from the reserved stack. -/
structure Layout : Prop where
  slabs_nowrap : (c.slabs s).base.toNat + (c.slabs s).len ≤ 2 ^ c.A.ptrBits
  slabs_res : ∀ q ∈ c.A.reserved 0 s, (c.slabs s).Disjoint q
  nowrap : ∀ x ∈ c.fixed s, x.1.base.toNat + x.1.len ≤ 2 ^ c.A.ptrBits
  disj : (c.fixed s).Pairwise (fun a b => (a.2 || b.2) → a.1.Disjoint b.1)
  slabs : ∀ x ∈ c.fixed s, x.1.Disjoint (c.slabs s)
  res : ∀ x ∈ c.fixed s, ∀ q ∈ c.A.reserved 0 s, x.1.Disjoint q

/-- The stack `env` keeps: out of line, the return address's slot (the
reserved stack); inline, none, since the frame the block is inlined into may
hold locals other threads share. -/
def keptStack : List Region := match c.form with
  | .inline => []
  | _ => c.A.reserved 0 s

/-- The regions a routine may read but not write, and those it may write. -/
def ro : List Region := ((c.regions s).filter (!·.2)).map (·.1)
def rw : List Region := ((c.regions s).filter (·.2)).map (·.1)

/-- The world a routine runs in, as far as it is not the routine's own
registers: the state's regions are the routine's (`regions`), which do not
wrap around, overlap where one is written, or overlap the return address. -/
structure World : Prop where
  rd : c.A.rd s = c.ro s
  wr : c.A.wr s = c.rw s
  nowrap : ∀ x ∈ c.regions s, x.1.base.toNat + x.1.len ≤ 2 ^ c.A.ptrBits
  disj : (c.regions s).Pairwise (fun a b => (a.2 || b.2) → a.1.Disjoint b.1)
  res : ∀ x ∈ c.regions s, ∀ r ∈ c.A.reserved 0 s, x.1.Disjoint r

/-- What the routine's arguments and constants must be, which its registers
hold: a pop's or push's class has a slab word and, inline, its offset is a
multiple of 8 and its mask an allowed one; a stack push's magazine is a
nonzero multiple of 8. (A slow path's offset names a word of the slab
whatever it is: `cls`.) -/
def argsOk : Prop := match c.form with
  | .inline => c.A.param.isSome ∧ (c.param s COFF).toNat = 8 * c.cls s ∧ c.cls s < CLASS_LIMIT ∧
      MaskOk (c.mask s)
  | .table => c.cls s < CLASS_LIMIT
  | .cas | .stackPop => True
  | .stackPush => (c.arg s 1).toNat ≠ 0 ∧ (c.arg s 1).toNat % 8 = 0

/-- The class's word in slab `i`. -/
def wordOf (i : Nat) : BitVec 64 := (c.mem s).readW (wordAddr (c.sym s SLABS) i (c.cls s)) 64

/-- A slab word of a pop's or push's class: zero, a marker, or a slot of a
magazine of the class's size that lies `Apart`. -/
def SlotOk (w : BitVec 64) : Prop :=
  WordOk (c.slots s) w ∧ (IsSlot (c.slots s) w → c.Apart s ⟨magBase (c.slots s) w, 8 * c.slots s⟩)

/-- Each slab's magazine of the class is its own: the magazines two slab
words name are disjoint. -/
def Exclusive : Prop := ∀ i < SLAB_COUNT, ∀ j < SLAB_COUNT, i ≠ j →
  IsSlot (c.slots s) (c.wordOf s i) → IsSlot (c.slots s) (c.wordOf s j) →
  Region.Disjoint ⟨magBase (c.slots s) (c.wordOf s i), 8 * c.slots s⟩
    ⟨magBase (c.slots s) (c.wordOf s j), 8 * c.slots s⟩

/-- The invariant the routine relies on, of memory, which every thread and
the drainer keep: for a pop or push, that of the class's word in every slab
(`SlotOk`), each slab's magazine its own (`Exclusive`), and, out of line,
that the class's mask in `MAG_MASKS` is an allowed one; for a stack push or
pop, that each slab's head at the offset is a marker or heads a stack whose
link slots lie `Apart` (so, for a push, apart from the magazine pushed: it
is on no stack); for a store-if-equal, that no slab's word at the offset is
`NO_CPU`, which it returns without a slab (no slab word or head of the
allocator is: each is zero, a marker or a multiple of 8). -/
def inv : Prop := match c.form with
  | .inline => (∀ i < SLAB_COUNT, c.SlotOk s (c.wordOf s i)) ∧ c.Exclusive s
  | .table => MaskOk (c.mask s) ∧ (∀ i < SLAB_COUNT, c.SlotOk s (c.wordOf s i)) ∧ c.Exclusive s
  | .cas => ∀ i < SLAB_COUNT, (c.wordOf s i).toNat ≠ NO_CPU
  | .stackPush | .stackPop => ∀ i < SLAB_COUNT, StackOk (c.mem s) (c.wordOf s i) (c.Apart s)

/-- The precondition beyond the signature's: the target names the thread
pointer and symbols, the arguments are as `argsOk` requires, the layout is
`Layout`, and the invariant holds. -/
def pre : Prop := c.A.tp.isSome ∧ c.A.sym.isSome ∧ c.argsOk s ∧ c.Layout s ∧ c.inv s

/-! ### Pop -/

/-- The pointer a pop takes and the magazine it leaves; `none` if the CPU number has
no slab or the magazine none to give (none, a marker, or empty). -/
def popResult : Option (BitVec 64 × Mag) := if c.InRange s then (c.mag s).pop else none

/-- A pop that took the pointer `x`: memory changed only in `rseq_cs` and the
slab word, which now names the slot below, and the magazine is the rest. -/
def Popped (s' : M.State) (x : BitVec 64) : Prop :=
  ∃ rest, c.popResult s = some (x, rest) ∧
    Unchanged [c.csField s, ⟨c.word s, 8⟩] (c.mem s) (c.A.mem s') ∧
    (c.A.mem s').readW (c.word s) 64 = c.w s - 8 ∧
    Mag.ofWord (c.slots s) (c.A.mem s') (c.w s - 8) = rest

/-- A pop that missed: memory changed only in `rseq_cs`. -/
def PopMissed (s' : M.State) : Prop :=
  c.popResult s = none ∧ Unchanged [c.csField s] (c.mem s) (c.A.mem s')

/-! ### Push -/

/-- The magazine a push of `x` leaves; `none` if the CPU number has no slab or the
magazine no free slot to spare (none, a marker, or full). -/
def pushResult (x : BitVec 64) : Option Mag :=
  if c.InRange s then (c.mag s).push (c.slots s) x else none

/-- A push of `x`: memory changed only in `rseq_cs`, the slab word, which
now names the slot above, and the free slot it named. -/
def Pushed (x : BitVec 64) (s' : M.State) : Prop :=
  ∃ rest, c.pushResult s x = some rest ∧
    Unchanged [c.csField s, ⟨c.word s, 8⟩, ⟨c.w s, 8⟩] (c.mem s) (c.A.mem s') ∧
    (c.A.mem s').readW (c.word s) 64 = c.w s + 8 ∧
    Mag.ofWord (c.slots s) (c.A.mem s') (c.w s + 8) = rest

/-- A push of `x` that missed: memory changed only in `rseq_cs`. -/
def PushMissed (x : BitVec 64) (s' : M.State) : Prop :=
  c.pushResult s x = none ∧ Unchanged [c.csField s] (c.mem s) (c.A.mem s')

/-! ### Accesses

The memory accesses of a run that is not aborted, in terms of the values it
reads (`Contract.trace`): another thread may change memory between them
(the drainer exchanging a slab word with a marker, say), so it matters
what the code stores as a function of what it read, not of memory on entry.
Both read `__rseq_offset` (`o`), arm `rseq_cs` (`d`), read the CPU number (`i`), and
then, for one with a slab, read the slab word once (`w`) and test it with
the mask (`k`, read from the table out of line); a pop then reads the slot
below (`v`) and commits `w - 8`; a push stores its pointer `x` in the free
slot, which no abstraction of the slab includes, and commits `w + 8`. The
commit is the last access and the only store to a slab word. -/

/-- A read and a write of `n` bytes. -/
def rd (a : Addr) (n v : Nat) : Event := ⟨a, n, false, v⟩
def wr (a : Addr) (n v : Nat) : Event := ⟨a, n, true, v⟩

/-- The accesses up to the slab word's, given the values read. -/
def headTrace (o d i w k : Nat) : List Event :=
  let a := c.tp s + BitVec.ofNat 64 o
  [rd (c.sym s RSEQ_OFFSET) 8 o, wr (a + BitVec.ofNat 64 RSEQ_CS) 8 d,
    rd (a + BitVec.ofNat 64 RSEQ_CPU_ID) 4 i] ++
  if i < SLAB_COUNT then
    rd (wordAddr (c.sym s SLABS) i (c.cls s)) 8 w ::
      (match c.form with | .table => [rd (c.maskAddr s) 4 k] | _ => [])
  else []

/-- The mask a routine tests with, given the table entry it read. -/
def testMask (k : Nat) : Nat := match c.form with
  | .inline => (c.param s MASK).toNat
  | _ => k

/-- The accesses of a pop. -/
def PopTrace (evs : List Event) : Prop :=
  ∃ o d i w k v : Nat, evs = c.headTrace s o d i w k ++
    if i < SLAB_COUNT ∧ BitVec.ofNat 32 w &&& BitVec.ofNat 32 (c.testMask s k) ≠ 0 then
      [rd (BitVec.ofNat 64 w - 8) 8 v, wr (wordAddr (c.sym s SLABS) i (c.cls s)) 8 (BitVec.ofNat 64 w - 8).toNat]
    else []

/-- The accesses of a push of `x`. -/
def PushTrace (x : BitVec 64) (evs : List Event) : Prop :=
  ∃ o d i w k : Nat, evs = c.headTrace s o d i w k ++
    if i < SLAB_COUNT ∧ BitVec.ofNat 32 (w + 8) &&& BitVec.ofNat 32 (c.testMask s k) ≠ 0 then
      [wr (BitVec.ofNat 64 w) 8 x.toNat, wr (wordAddr (c.sym s SLABS) i (c.cls s)) 8 (BitVec.ofNat 64 w + 8).toNat]
    else []

/-! ### The slow path's

Each reads the CPU number and, if it has a slab, the word once (`w`). Then
a store-if-equal stores its third argument if `w` is its second; a stack
push, unless `w` is a marker, stores `w` in the magazine's link slot, which
nothing else can see, and commits the magazine's address; a stack pop,
unless `w` is zero or a marker, reads the link slot of the magazine `w`
names, the head it read once, and commits what it holds. -/

/-- The accesses of a store-if-equal of `x` if the word is `e`. -/
def CasTrace (e x : BitVec 64) (evs : List Event) : Prop :=
  ∃ o d i w : Nat, evs = c.headTrace s o d i w 0 ++
    if i < SLAB_COUNT ∧ w = e.toNat then [wr (wordAddr (c.sym s SLABS) i (c.cls s)) 8 x.toNat]
    else []

/-- The accesses of a stack push of the magazine `g`. -/
def StackPushTrace (g : BitVec 64) (evs : List Event) : Prop :=
  ∃ o d i w : Nat, evs = c.headTrace s o d i w 0 ++
    if i < SLAB_COUNT ∧ w % 2 = 0 then
      [wr g 8 w, wr (wordAddr (c.sym s SLABS) i (c.cls s)) 8 g.toNat]
    else []

/-- The accesses of a stack pop: the link read is at the head read. -/
def StackPopTrace (evs : List Event) : Prop :=
  ∃ o d i w v : Nat, evs = c.headTrace s o d i w 0 ++
    if i < SLAB_COUNT ∧ w ≠ 0 ∧ w % 2 = 0 then
      [rd (BitVec.ofNat 64 w) 8 v, wr (wordAddr (c.sym s SLABS) i (c.cls s)) 8 v]
    else []

end Ctx

/-! ## The contracts -/

/-- The contract of a routine `c` with the postcondition `post`, what it may
leave at its slow path's exit (`exit`), at an abort (`abort`), and its
accesses (`trace`): its signature's, its memory `Ctx.regions`, and the
precondition `Ctx.pre`.

Wherever it may be aborted, the routine has kept the invariant (`abort`).
A retry starts where the thread resumes after the abort, and `env` is what
other threads, the drainer and the operating system may have done
meanwhile: changed memory, but not `__rseq_offset` (glibc sets it once) or
the stack the calling convention reserves (the thread's own), keeping the
invariant. The thread's permissions there are the routine's regions,
recomputed from memory then: the model's account of what the routine may
access, not an assumption about other threads. The routine proves its
precondition there from these, its arguments, which it keeps
(`Sig.contract`'s `abort`), and its registers and constants, which the
abort keeps (`Target.rely`). -/
def contract {M : ISA} (c : Ctx M) (post : M.State → M.State → Prop)
    (exit : String → M.State → M.State → Prop) (abort : M.State → M.State → Prop)
    (trace : M.State → List Event → Prop) : Contract M :=
  let A := c.A.withExtra (some c.regions)
  let post' := Curry.const (fun _ _ _ => True) _
  let x : Sig.Extra M := {
    pre := c.pre
    post := post
    exit := exit
    abort := fun s p => abort s p ∧ c.inv p
    trace := trace
    env := fun p p' => c.A.rd p' = c.ro p' ∧ c.A.wr p' = c.rw p' ∧
      Kept (c.rsVar p :: c.keptStack p) (c.mem p) (c.mem p') ∧ (c.inv p → c.inv p') }
  c.sig.contract A (post := post') (x := x)

/-- `slab_pop` inline: `fn<const CLASS: usize, const MASK: u32>() -> usize`. -/
def popInlineSig : Sig where
  params := []
  ret := some .usize

/-- `slab_push` inline: `fn<const CLASS: usize, const MASK: u32>(p: usize) -> bool`. -/
def pushInlineSig : Sig where
  params := [("p", .int .usize)]

/-- `slab_pop` out of line: `fn(class: usize) -> usize`. -/
def popSig : Sig where
  params := [("class", .int .usize)]
  ret := some .usize

/-- `slab_push` out of line: `fn(class: usize, p: usize)`. -/
def pushSig : Sig where
  params := [("class", .int .usize), ("p", .int .usize)]

/-- Inline pop: returns the top pointer of the magazine and removes it, or
returns 0 and changes nothing the abstraction sees. -/
def popInlineContract {M : ISA} (A : Abi M) : Contract M :=
  let c : Ctx M := ⟨A, .inline, popInlineSig⟩
  contract c
    (post := fun s s' => (∃ x, c.Popped s s' x ∧ A.ret s' = x) ∨ (c.PopMissed s s' ∧ A.ret s' = 0))
    (exit := fun _ _ _ => False)
    (abort := fun s p => Unchanged [c.csField s] (c.mem s) (A.mem p))
    (trace := c.PopTrace)

/-- Where a push may be aborted, every slab's magazine of the class is as
it was: memory has changed only in `rseq_cs` and the free slot, which no
slab's magazine holds. -/
def PushAbort {M : ISA} (c : Ctx M) (s p : M.State) : Prop :=
  Unchanged [c.csField s, ⟨c.w s, 8⟩] (c.mem s) (c.A.mem p) ∧
    ∀ i < SLAB_COUNT, Mag.ofWord (c.slots s) (c.A.mem p) (c.wordOf s i) =
      Mag.ofWord (c.slots s) (c.mem s) (c.wordOf s i)

/-- Inline push of `p`: pushes it, or exits to `MISS` (the function returns
`false`) and changes nothing the abstraction sees. -/
def pushInlineContract {M : ISA} (A : Abi M) : Contract M :=
  let c : Ctx M := ⟨A, .inline, pushInlineSig⟩
  contract c
    (post := fun s s' => c.Pushed s (c.arg s 0) s')
    (exit := fun t s s' => t = MISS ∧ c.PushMissed s (c.arg s 0) s')
    (abort := PushAbort c)
    (trace := fun s => c.PushTrace s (c.arg s 0))

/-- Out-of-line pop of `class`: returns the top pointer of the magazine and
removes it, or tail-calls the slow path `SLOW` with `class`, having changed
nothing the abstraction sees. -/
def popContract {M : ISA} (A : Abi M) : Contract M :=
  let c : Ctx M := ⟨A, .table, popSig⟩
  contract c
    (post := fun s s' => ∃ x, c.Popped s s' x ∧ A.ret s' = x)
    (exit := fun t s s' => t = SLOW ∧ c.PopMissed s s')
    (abort := fun s p => Unchanged [c.csField s] (c.mem s) (A.mem p))
    (trace := c.PopTrace)

/-- Out-of-line push of `p` of `class`: pushes it, or tail-calls the slow
path `SLOW` with `class` and `p`, having changed nothing the abstraction
sees. -/
def pushContract {M : ISA} (A : Abi M) : Contract M :=
  let c : Ctx M := ⟨A, .table, pushSig⟩
  contract c
    (post := fun s s' => c.Pushed s (c.arg s 1) s')
    (exit := fun t s s' => t = SLOW ∧ c.PushMissed s (c.arg s 1) s')
    (abort := PushAbort c)
    (trace := fun s => c.PushTrace s (c.arg s 1))

/-! ### The slow path's routines

Out of line only. Each takes a byte offset in a slab, of which it uses the
word it falls in (`Ctx.cls`), and returns `NO_CPU` if the thread's CPU
number has no slab, having changed nothing but `rseq_cs`. -/

/-- `slab_cas`: `fn(offset: usize, expected: usize, new: usize) -> usize`. -/
def casSig : Sig where
  params := [("offset", .int .usize), ("expected", .int .usize), ("new", .int .usize)]
  ret := some .usize

/-- `slab_stack_push`: `fn(offset: usize, magazine: usize) -> usize`. -/
def stackPushSig : Sig where
  params := [("offset", .int .usize), ("magazine", .int .usize)]
  ret := some .usize

/-- `slab_stack_pop`: `fn(offset: usize) -> usize`. -/
def stackPopSig : Sig where
  params := [("offset", .int .usize)]
  ret := some .usize

/-- No slab for the CPU number: `NO_CPU`, and only `rseq_cs` changed. -/
def NoCpu {M : ISA} (c : Ctx M) (s s' : M.State) : Prop :=
  ¬ c.InRange s ∧ c.A.ret s' = BitVec.ofNat 64 NO_CPU ∧
    Unchanged [c.csField s] (c.mem s) (c.A.mem s')

/-- Store-if-equal: returns the word it read, which it replaces with `new`
if it was `expected`; otherwise it changes nothing but `rseq_cs`. -/
def casContract {M : ISA} (A : Abi M) : Contract M :=
  let c : Ctx M := ⟨A, .cas, casSig⟩
  contract c
    (post := fun s s' => NoCpu c s s' ∨ (c.InRange s ∧ A.ret s' = c.w s ∧
      ((c.w s = c.arg s 1 ∧ Unchanged [c.csField s, ⟨c.word s, 8⟩] (c.mem s) (A.mem s') ∧
          (A.mem s').readW (c.word s) 64 = c.arg s 2) ∨
        (c.w s ≠ c.arg s 1 ∧ Unchanged [c.csField s] (c.mem s) (A.mem s')))))
    (exit := fun _ _ _ => False)
    (abort := fun s p => Unchanged [c.csField s] (c.mem s) (A.mem p))
    (trace := fun s => c.CasTrace s (c.arg s 1) (c.arg s 2))

/-- Stack push of `magazine`: returns the head it read; if that is a marker,
changes nothing but `rseq_cs`; otherwise the magazine's link slot holds
that head and the head is the magazine: the stack is the magazine on top of
what it was. Where it may be aborted, every slab's stack at the offset is as
it was. -/
def stackPushContract {M : ISA} (A : Abi M) : Contract M :=
  let c : Ctx M := ⟨A, .stackPush, stackPushSig⟩
  contract c
    (post := fun s s' => NoCpu c s s' ∨ (c.InRange s ∧ A.ret s' = c.w s ∧
      (((c.w s).toNat % 2 = 1 ∧ Unchanged [c.csField s] (c.mem s) (A.mem s')) ∨
        ((c.w s).toNat % 2 = 0 ∧
          Unchanged [c.csField s, ⟨c.word s, 8⟩, c.link s] (c.mem s) (A.mem s') ∧
          (A.mem s').readW (c.word s) 64 = c.arg s 1 ∧
          ∀ l, Chain (c.mem s) (c.w s) l → Chain (A.mem s') (c.arg s 1) (c.arg s 1 :: l)))))
    (exit := fun _ _ _ => False)
    (abort := fun s p => Unchanged [c.csField s, c.link s] (c.mem s) (A.mem p) ∧
      ∀ i < SLAB_COUNT, ∀ l, Chain (c.mem s) (c.wordOf s i) l → Chain (A.mem p) (c.wordOf s i) l)
    (trace := fun s => c.StackPushTrace s (c.arg s 1))

/-- Stack pop: returns the head it read; if that is zero (empty) or a
marker, changes nothing but `rseq_cs`; otherwise the head is what the top
magazine's link slot held: the stack is the rest. -/
def stackPopContract {M : ISA} (A : Abi M) : Contract M :=
  let c : Ctx M := ⟨A, .stackPop, stackPopSig⟩
  contract c
    (post := fun s s' => NoCpu c s s' ∨ (c.InRange s ∧ A.ret s' = c.w s ∧
      ((((c.w s).toNat = 0 ∨ (c.w s).toNat % 2 = 1) ∧ Unchanged [c.csField s] (c.mem s) (A.mem s')) ∨
        ((c.w s).toNat ≠ 0 ∧ (c.w s).toNat % 2 = 0 ∧
          Unchanged [c.csField s, ⟨c.word s, 8⟩] (c.mem s) (A.mem s') ∧
          (A.mem s').readW (c.word s) 64 = (c.mem s).readW (c.w s) 64 ∧
          ∀ l, Chain (c.mem s) (c.w s) l → Chain (A.mem s') ((c.mem s).readW (c.w s) 64) l.tail))))
    (exit := fun _ _ _ => False)
    (abort := fun s p => Unchanged [c.csField s] (c.mem s) (A.mem p))
    (trace := c.StackPopTrace)

/-! ### The CPU number

`current_cpu`, for the Rust front end, which cannot read the thread pointer
without assembly: the thread's `cpu_id` as the routines read it. It is not a
restartable sequence and arms nothing: it reads `__rseq_offset` and then
`cpu_id`, once each, and writes nothing. Its answer may be stale by the time
the caller uses it; the routines read the CPU number again. -/

/-- `current_cpu`: `fn() -> u32`. -/
def cpuSig : Sig where
  params := []
  ret := some .u32

/-- The routines' view of the state, of which `current_cpu` uses only the
rseq area's fields, which no routine's form changes (`Ctx.rsVar`,
`Ctx.cpuField`, `Ctx.cpu`). -/
def cpuView {M : ISA} (A : Abi M) : Ctx M := ⟨A, .cas, cpuSig⟩

/-- What `current_cpu` may read: `__rseq_offset` and the thread's `cpu_id`. -/
def cpuRegions {M : ISA} (A : Abi M) (s : M.State) : List (Region × Bool) :=
  [((cpuView A).rsVar s, false), ((cpuView A).cpuField s, false)]

/-- `current_cpu` returns `cpu_id`, zero-extended, and changes no memory. -/
def cpuContract {M : ISA} (A : Abi M) : Contract M :=
  let x : Sig.Extra M := {
    pre := fun _ => A.tp.isSome ∧ A.sym.isSome
    post := fun s s' => A.ret s' = BitVec.ofNat 64 ((cpuView A).cpu s) ∧ A.mem s' = A.mem s }
  cpuSig.contract (A.withExtra (some (cpuRegions A))) (post := Curry.const (fun _ _ _ => True) _)
    (x := x)

/-! ## The functions

Their documentation states what `Ctx.pre` and `Ctx.regions` require of the
caller, in Rust terms. -/

/-- How the documentation names the stack an out-of-line routine (`false`)
and an inline one (`true`) may not touch: the calling convention's reserved
memory (`Abi.reservedDoc`). -/
def stackDoc (inline : Bool) : String :=
  if inline then "the 128 bytes of stack below the stack pointer (the red zone) or the 8 bytes at it"
  else "the return address on the stack"

/-- The memory every routine reads or writes whatever its CPU number, as
the documentation names it (`Ctx.fixed`), for a table's mask entry `mask`
and a pushed magazine's link slot `link`. -/
def fixedNames (mask link : Option String) : List String :=
  ["`SLABS`", "the thread's `rseq_cs` and `cpu_id`", "`__rseq_offset`"] ++ mask.toList ++ link.toList

/-- That the slabs and what `Ctx.fixed` names lie apart (`Ctx.Layout`). -/
def layoutDoc (inline : Bool) (mask link : Option String) : String :=
  s!"None of {englishList "and" (fixedNames mask link)} may overlap {stackDoc inline} or wrap \
    around the end of the address space, nor overlap another of them, but where both are only \
    read ({englishList "and" (["`cpu_id`", "`__rseq_offset`"] ++ mask.toList)}) \
    (`VG.Spec.Slab.Ctx.Layout`)."

/-- That every thread and the drainer keep the invariant while a routine
may run, which a retry relies on (`Spec.Slab.contract`'s `env`). -/
def keepDoc : String :=
  "Every thread and the drainer must keep this from when the function is called until it \
    returns: the function may be aborted and restarted on any CPU in between \
    (`VG.Spec.Slab.Ctx.inv`)."

/-- What every pop and push requires of the thread and of the slabs; `cls`
and `size` name the class and how the routine is told its magazines' size,
`mask` the table entry it reads it from, if any. -/
def safety (inline : Bool) (cls size : String) (mask : Option String) : List String := [
  "glibc must have registered the calling thread's rseq area, as it does before the thread \
    can run code that allocates.",
  s!"{cls} must be below {CLASS_LIMIT}, and {size} the mask `8 * n - 16` of that class's \
    magazines, of `n = 2^k` slots for {MAG_LOG_MIN} ≤ k ≤ {MAG_LOG_MAX}.",
  layoutDoc inline mask none,
  s!"In each of the {SLAB_COUNT} slabs of `SLABS`, the class's slab word must be 0, a marker \
    (a word whose low {MARKER_SHIFT} bits are 1), or the address of a slot other than slot 0 \
    of a magazine of the class (`8 * n` bytes, aligned to `8 * n`), whose pointers are not 0 \
    (a pop returns 0 for none) and which does not overlap \
    {englishList "or" (fixedNames mask none ++ [stackDoc inline])}, the magazine another \
    slab's word of the class names, nor wrap around the end of the address space. " ++ keepDoc]

/-- What the slow path's routines require of the thread and the slab, and,
for a stack's, of the stacks. -/
def slowSafety (stack : Bool) (push : Bool) : List String :=
  let link := if push then some "`magazine[0]`" else none
  ["glibc must have registered the calling thread's rseq area, as it does before the thread \
    can run code that allocates.",
    layoutDoc false none link] ++
  (if stack then [] else
    [s!"In each of the {SLAB_COUNT} slabs of `SLABS`, the word at `offset` (rounded down to a \
      multiple of 8, within the slab) must not be `NO_CPU` (2), which the function returns \
      when the CPU number has no slab; no slab word or head is (each is 0, a marker or a \
      multiple of 8). " ++ keepDoc]) ++
  (if stack then
    [s!"In each of the {SLAB_COUNT} slabs of `SLABS`, the head at `offset` (rounded down to a \
      multiple of 8, within the slab) must be a marker (a word whose low {MARKER_SHIFT} bits \
      are 1) or head a stack of magazines: 0, or the address, a nonzero multiple of 8, of a \
      magazine whose link slot holds the next head. No link slot of those stacks may overlap \
      {englishList "or" (fixedNames none link ++ [stackDoc false])}, nor wrap around the end \
      of the address space. " ++ keepDoc]
  else []) ++
  (if push then ["`magazine` must be a nonzero multiple of 8, and the magazine must be the \
    caller's: on no stack."] else [])

/-- The layout the routines are proved for, for the Rust around them: the
slabs, the classes, the markers, the fields of `struct rseq` they use, and
the mask of each allowed magazine size (`MAG_MASK_<slots>`). -/
def rustConsts : List (String × IntTy × Nat) :=
  [("SLAB_SHIFT", .u32, SLAB_SHIFT), ("SLAB_BYTES", .usize, SLAB_BYTES),
    ("SLAB_COUNT", .usize, SLAB_COUNT), ("CLASS_LIMIT", .usize, CLASS_LIMIT),
    ("WORD_BYTES", .usize, 8), ("MARKER_SHIFT", .u32, MARKER_SHIFT),
    ("RSEQ_CS", .usize, RSEQ_CS), ("RSEQ_CPU_ID", .usize, RSEQ_CPU_ID),
    ("MAG_LOG_MIN", .u32, MAG_LOG_MIN), ("MAG_LOG_MAX", .u32, MAG_LOG_MAX),
    ("NO_CPU", .usize, NO_CPU)] ++
  (List.range (MAG_LOG_MAX - MAG_LOG_MIN + 1)).map fun i =>
    (s!"MAG_MASK_{2 ^ (MAG_LOG_MIN + i)}", .u32, magMask (2 ^ (MAG_LOG_MIN + i)))

def popInlineApi : Api where
  module := "slab"
  name := "slab_pop"
  sig := popInlineSig
  contracts := some fun A _ => popInlineContract A
  summary := "Pops a pointer from the magazine of class `CLASS` of the calling thread's CPU, or \
    returns 0 if the CPU number has no slab or the magazine none to give.\n\n\
    Contract: `VG.Spec.Slab.popInlineContract`."
  safety := safety true "`CLASS`" "`MASK`" none
  operands := [(RSEQ_OFFSET, .sym (path RSEQ_OFFSET)), (SLABS, .sym (path SLABS)),
    (COFF, .const "CLASS * 8"), (MASK, .const "MASK")]
  generics := "const CLASS: usize, const MASK: u32"
  rustConsts := rustConsts

def pushInlineApi : Api where
  module := "slab"
  name := "slab_push"
  sig := pushInlineSig
  contracts := some fun A _ => pushInlineContract A
  summary := "Pushes `p` on the magazine of class `CLASS` of the calling thread's CPU and \
    returns `true`, or returns `false` if the CPU number has no slab or the magazine no free slot to \
    spare.\n\n\
    Contract: `VG.Spec.Slab.pushInlineContract`."
  safety := safety true "`CLASS`" "`MASK`" none
  operands := [(RSEQ_OFFSET, .sym (path RSEQ_OFFSET)), (SLABS, .sym (path SLABS)),
    (COFF, .const "CLASS * 8"), (MASK, .const "MASK")]
  generics := "const CLASS: usize, const MASK: u32"
  rustConsts := rustConsts

def popApi : Api where
  module := "slab"
  name := "slab_pop"
  sig := popSig
  contracts := some fun A _ => popContract A
  summary := "Pops a pointer from the magazine of class `class` of the calling thread's CPU and \
    returns it, or tail-calls `front::pop_slow(class)` if the CPU number has no slab or the magazine \
    none to give. `pop_slow` starts with the thread's `rseq_cs` still pointing at this \
    function's sequence, which it has left: the kernel clears it at the next preemption, and \
    `pop_slow` may arm its own.\n\n\
    Contract: `VG.Spec.Slab.popContract`."
  safety := safety false "`class`" "`MAG_MASKS[class]`" (some "the mask's entry `MAG_MASKS[class]`")
  operands := [(RSEQ_OFFSET, .sym (path RSEQ_OFFSET)), (SLABS, .sym (path SLABS)),
    (MAG_MASKS, .sym (path MAG_MASKS)), (SLOW, .sym (path "pop_slow"))]
  rustConsts := rustConsts

def pushApi : Api where
  module := "slab"
  name := "slab_push"
  sig := pushSig
  contracts := some fun A _ => pushContract A
  summary := "Pushes `p` on the magazine of class `class` of the calling thread's CPU, or \
    tail-calls `front::push_slow(class, p)` if the CPU number has no slab or the magazine no free \
    slot to spare. `push_slow` starts with the thread's `rseq_cs` still pointing at this \
    function's sequence, which it has left: the kernel clears it at the next preemption, and \
    `push_slow` may arm its own.\n\n\
    Contract: `VG.Spec.Slab.pushContract`."
  safety := safety false "`class`" "`MAG_MASKS[class]`" (some "the mask's entry `MAG_MASKS[class]`")
  operands := [(RSEQ_OFFSET, .sym (path RSEQ_OFFSET)), (SLABS, .sym (path SLABS)),
    (MAG_MASKS, .sym (path MAG_MASKS)), (SLOW, .sym (path "push_slow"))]
  rustConsts := rustConsts

def casApi : Api where
  module := "slab"
  name := "slab_cas"
  sig := casSig
  contracts := some fun A _ => casContract A
  summary := "Stores `new` in the calling thread's CPU's slab word at `offset` if it holds \
    `expected`, and returns the word it read there; returns `NO_CPU` (2) if the CPU number \
    has no slab.\n\n\
    Contract: `VG.Spec.Slab.casContract`."
  safety := slowSafety false false
  operands := [(RSEQ_OFFSET, .sym (path RSEQ_OFFSET)), (SLABS, .sym (path SLABS))]
  rustConsts := rustConsts

def stackPushApi : Api where
  module := "slab"
  name := "slab_stack_push"
  sig := stackPushSig
  contracts := some fun A _ => stackPushContract A
  summary := "Pushes `magazine` on the calling thread's CPU's stack whose head is at `offset`, \
    unless the head is a marker, and returns the head it read; returns `NO_CPU` (2) if the \
    CPU number has no slab.\n\n\
    Contract: `VG.Spec.Slab.stackPushContract`."
  safety := slowSafety true true
  operands := [(RSEQ_OFFSET, .sym (path RSEQ_OFFSET)), (SLABS, .sym (path SLABS))]
  rustConsts := rustConsts

def stackPopApi : Api where
  module := "slab"
  name := "slab_stack_pop"
  sig := stackPopSig
  contracts := some fun A _ => stackPopContract A
  summary := "Pops the top magazine off the calling thread's CPU's stack whose head is at \
    `offset`, unless the stack is empty (0) or the head a marker, and returns the head it \
    read; returns `NO_CPU` (2) if the CPU number has no slab.\n\n\
    Contract: `VG.Spec.Slab.stackPopContract`."
  safety := slowSafety true false
  operands := [(RSEQ_OFFSET, .sym (path RSEQ_OFFSET)), (SLABS, .sym (path SLABS))]
  rustConsts := rustConsts

def currentCpuApi : Api where
  module := "slab"
  name := "current_cpu"
  sig := cpuSig
  contracts := some fun A _ => cpuContract A
  summary := "Returns the calling thread's CPU number as the slab routines read it: its rseq \
    area's `cpu_id`, zero-extended. A number not below `SLAB_COUNT` has no slab (an \
    unregistered thread's reads as one). Not a restartable sequence: the thread may move to \
    another CPU before the caller uses the answer.\n\n\
    Contract: `VG.Spec.Slab.cpuContract`."
  safety := [
    "glibc must have registered the calling thread's rseq area, as it does before the thread \
      can run code that allocates.",
    "Neither `__rseq_offset` nor the thread's `cpu_id` may overlap the stack the call uses (the \
      return address on the stack, or, inline, the 128 bytes below the stack pointer and the 8 \
      bytes at it) or wrap around the end of the address space."]
  operands := [(RSEQ_OFFSET, .sym (path RSEQ_OFFSET))]

end VG.Spec.Slab
