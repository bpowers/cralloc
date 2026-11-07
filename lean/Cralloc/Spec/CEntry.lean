module

public import Cralloc.Spec.Slab

/-!
# The C entry points: `malloc`, `free` and the sized free

**Trusted** (as every file in `Spec/`). The contracts of the routines that
serve C's `malloc(size)`, `calloc(n, size)`, `free(ptr)` and `free_sized(ptr, size)`, each
one restartable sequence that finds the
class first and then pops or pushes as the routines of `Spec/Slab.lean` do,
with the class's mask read from the table past the slabs. See docs/design.md, "Interfaces", and
docs/research/roofline.md, "The C entry points".

## What a routine reads to find the class

None of these routines is given a class. Each finds one from memory the
Rust side keeps, and is proved safe for every content of it:

* `malloc` reads the byte `C_SIZES[(size + 7) / 8]` of a table of
  `MALLOC_MAX / 8 + 1` bytes, for a size up to `MALLOC_MAX`; a larger one
  takes the slow path.
* `free` reads the published shift and base index of the heap, two words
  of its mirror (`Mirror`) beside the slabs, computes the region
  index `(ptr >> shift) - base_index` (the shift taken modulo 64, as the
  processor takes a count in `cl`), and, if it is below `REGION_COUNT`,
  reads the class byte of the region table `REGION_DISP` bytes above
  `SLABS`. Null returns at once.
* The sized free predicts the class from the size, the byte
  `C_SIZED_SIZES[size mod 2^16]` of a table of `SIZED_LEN` bytes, finds the
  region's byte as `free` does, and goes on only if the two agree. So it
  acts exactly where `free` would, whatever the size says; any disagreement,
  or a region index out of range, takes the slow path.

The class byte `k` found names the class's word in slab 0, `SLABS + 8 k`,
and the class's mask, the low 32 bits of the entry `MASK_DISP` bytes above
that word. Any byte is a class here:
the invariant (`Ctx.inv`) holds of every one of the `MASK_COUNT` entries,
and the Rust side keeps every entry that is not a small class's at zero,
which makes the routine miss.

What it means for these values to be the right ones (the table bytes the
classes of their sizes, the published words those of the heap, the region
byte the class of every block the region holds) is the Rust side's to
keep; the routines need none of it for memory safety, and their contracts
state what they did in terms of the values they read.

## Mirrors

The frees read the heap's shift and base index from a mirror (`Mirror`), a
copy beside the slabs. It is zero until the Rust side publishes it, and a
zero mirror sends every pointer to the slow path (the index is the pointer
itself, which is far above `REGION_COUNT` for any address the heap can
have). There is one mirror, C's. A family of frees that must stay on its
slow path until the Rust side decides it may leave it (as C++'s `delete`
forms must when a program replaces `operator new`) would get a mirror of its
own, which the Rust side leaves zero. The routines are proved safe whatever
a mirror holds.

## Restarts

Each routine is one sequence, armed first: after the arm it reads the CPU
number, then finds the class, then pops or pushes. If it is aborted it
starts again from the arm and reads everything again; nothing it reads
before the commit is kept across an abort, so a retry may find another
class (a table being written in between, say) and is proved for whatever
it finds. The slab invariant, of every class, is what other threads keep
meanwhile (`env`).
-/

@[expose] public section

namespace VG.Spec.CEntry

open VG.Spec.Slab

/-! ## Layout -/

/-- The first mirror of the published heap's region shift (32 bits) and base
index (64 bits), `base >> shift`: just past the slabs, 1024 bytes into the
page of the masks' table, a page offset no slab word has. The mirrors follow
one another, 16 bytes each (`Mirror.disp`). -/
def SHIFT_DISP : Nat := 33555456
def BASE_DISP : Nat := 33555464
def MIRROR_BYTES : Nat := 16

/-- Whose copy of the heap's shift and base index a free reads. -/
inductive Mirror
  /-- C's `free` and `free_sized`. -/
  | c
  deriving DecidableEq, Repr

def Mirror.index : Mirror → Nat
  | .c => 0

def MIRROR_COUNT : Nat := 1

/-- The functions a mirror serves, as the documentation names them. -/
def Mirror.doc : Mirror → String
  | .c => "C's `free` and `free_sized`"

/-- The mirror's shift word, above `SLABS`; its base index word is 8 bytes on. -/
def Mirror.disp (m : Mirror) : Nat := SHIFT_DISP + MIRROR_BYTES * m.index
/-- The region table: one class byte per region, `REGION_COUNT` of them,
starting 2048 bytes into the page after the masks' table. -/
def REGION_DISP : Nat := 33560576
def REGION_COUNT : Nat := 4096
/-- Where each size table starts within its static: 2048 bytes in, so that
the bytes of small sizes have page offsets no slab word has. -/
def TABLE_OFF : Nat := 2048
/-- The sizes `malloc`'s table covers, and its length. -/
def MALLOC_MAX : Nat := 32768
def MALLOC_LEN : Nat := MALLOC_MAX / 8 + 1
/-- The sized free's table: one byte per value of the size's low 16 bits. -/
def SIZED_LEN : Nat := 65536
/-- The totals `calloc`'s table covers, one byte per total, and its length. -/
def CALLOC_MAX : Nat := 1024
def CALLOC_LEN : Nat := CALLOC_MAX + 1

theorem shift_disp_eq : SHIFT_DISP = SLAB_COUNT * SLAB_BYTES + 1024 := by decide
theorem base_disp_eq : BASE_DISP = SHIFT_DISP + 8 := by decide
theorem region_disp_eq : REGION_DISP = SLAB_COUNT * SLAB_BYTES + SLAB_BYTES + 2048 := by decide

/-- The names of the size tables the code reads, and of the slow paths. -/
def SIZES : String := "C_SIZES"
def SIZED : String := "C_SIZED_SIZES"
def CALLOC_SIZES : String := "C_CALLOC_SIZES"
/-- `calloc`'s zeroing tail, which a pop that hit jumps to. -/
def ZERO : String := "zero"

/-! ## The routines and their view of the state -/

/-- Which routine. -/
inductive Kind
  /-- `c_malloc(size)`. -/
  | malloc
  /-- `c_free(ptr)`. -/
  | free
  /-- `c_free_sized(ptr, size)`. -/
  | freeSized
  /-- `c_calloc(n, size)`. -/
  | calloc
  deriving DecidableEq, Repr

structure Ctx (M : ISA) where
  A : Abi M
  kind : Kind
  sig : Sig
  /-- The mirror a free reads; `malloc`'s is never read. -/
  mirror : Mirror

namespace Ctx

variable {M : ISA} (c : Ctx M) (s : M.State)

/-- The slab routines' view of the same state, for the thread's rseq area
and the symbols, which do not depend on the routine. -/
def slab : Spec.Slab.Ctx M := ⟨c.A, .cas, c.sig⟩

def mem : Mem := c.A.mem s
def sym (n : String) : Addr := c.slab.sym s n
def arg (i : Nat) : BitVec 64 := c.slab.arg s i

def csField : Region := c.slab.csField s
def cpuField : Region := c.slab.cpuField s
def rsVar : Region := c.slab.rsVar s
def cpu : Nat := c.slab.cpu s

def InRange : Prop := c.cpu s < SLAB_COUNT

instance : Decidable (c.InRange s) := by unfold InRange; infer_instance

/-- The size and the pointer the routine is given. -/
def size : BitVec 64 := match c.kind with
  | .malloc => c.arg s 0
  | .free => 0
  | .freeSized => c.arg s 1
  | .calloc => c.arg s 0 * c.arg s 1
def ptr : BitVec 64 := match c.kind with
  | .malloc | .calloc => 0
  | .free | .freeSized => c.arg s 0
/-- Whether `calloc`'s product overflows 64 bits. -/
def overflows : Prop := 2 ^ 64 ≤ (c.arg s 0).toNat * (c.arg s 1).toNat

instance : Decidable (c.overflows s) := by unfold overflows; infer_instance

/-- The byte at `a`. -/
def byteAt (a : Addr) : Nat := ((c.mem s) a).toNat

/-- The memory the routine reads to find the class: the slabs' masks'
table, the size table, the published words and the region table. -/
def slabs : Region := ⟨c.sym s SLABS, SLAB_COUNT * SLAB_BYTES⟩
def maskTable : Region := ⟨c.sym s SLABS + BitVec.ofNat 64 MASK_DISP, 8 * MASK_COUNT⟩
def published : Region := ⟨c.sym s SLABS + BitVec.ofNat 64 c.mirror.disp, 16⟩
def regionTable : Region := ⟨c.sym s SLABS + BitVec.ofNat 64 REGION_DISP, REGION_COUNT⟩
def mallocTable : Region := ⟨c.sym s SIZES + BitVec.ofNat 64 TABLE_OFF, MALLOC_LEN⟩
def sizedTable : Region := ⟨c.sym s SIZED + BitVec.ofNat 64 TABLE_OFF, SIZED_LEN⟩
def callocTable : Region := ⟨c.sym s CALLOC_SIZES + BitVec.ofNat 64 TABLE_OFF, CALLOC_LEN⟩

/-- The region index of the pointer, from the published words. -/
def shift : Nat := ((c.mem s).readW (c.published s).base 32).toNat % 64
def baseIndex : BitVec 64 := (c.mem s).readW ((c.published s).base + 8) 64
def regionIndex : BitVec 64 := (c.ptr s >>> c.shift s) - c.baseIndex s
/-- The region's byte, if the index is in range. -/
def regionByte : Nat := c.byteAt s ((c.regionTable s).base + c.regionIndex s)

/-- The class `malloc`'s table gives a size up to `MALLOC_MAX`, and the one
the sized free's predicts. -/
def mallocClass : Nat :=
  c.byteAt s ((c.mallocTable s).base + BitVec.ofNat 64 (((c.size s).toNat + 7) / 8))
def predicted : Nat := c.byteAt s ((c.sizedTable s).base + BitVec.ofNat 64 ((c.size s).toNat % SIZED_LEN))
/-- The class `calloc`'s table gives a total up to `CALLOC_MAX`. -/
def callocClass : Nat := c.byteAt s ((c.callocTable s).base + BitVec.ofNat 64 (c.size s).toNat)

/-- The class the routine works on, if it finds one: for `malloc`, its
table's for a size up to `MALLOC_MAX`; for `free`, the region's of a
pointer that is not null, if its index is in range; for the sized free, the
predicted one, if it is the region's. -/
def cls? : Option Nat := match c.kind with
  | .malloc => if (c.size s).toNat ≤ MALLOC_MAX then some (c.mallocClass s) else none
  | .free => if (c.ptr s).toNat ≠ 0 ∧ (c.regionIndex s).toNat < REGION_COUNT then
      some (c.regionByte s) else none
  | .freeSized => if (c.regionIndex s).toNat < REGION_COUNT ∧ c.regionByte s = c.predicted s then
      some (c.predicted s) else none
  | .calloc => if ¬ c.overflows s ∧ (c.size s).toNat ≤ CALLOC_MAX then some (c.callocClass s)
      else none

/-! ### A class's words, mask and magazines -/

/-- The class `k`'s mask, and the slots of its magazines. -/
def maskOf (k : Nat) : Nat := ((c.mem s).readW (c.sym s SLABS + BitVec.ofNat 64 (8 * k + MASK_DISP)) 32).toNat
def slotsOf' (k : Nat) : Nat := slotsOf (c.maskOf s k)

/-- The class `k`'s word in slab `i`, and in the thread's CPU's slab. -/
def wordOf (k i : Nat) : BitVec 64 := (c.mem s).readW (wordAddr (c.sym s SLABS) i k) 64
def word (k : Nat) : Addr := wordAddr (c.sym s SLABS) (c.cpu s) k
def w (k : Nat) : BitVec 64 := (c.mem s).readW (c.word s k) 64

def mag (k : Nat) : Mag := Mag.ofWord (c.slotsOf' s k) (c.mem s) (c.w s k)
def magRegion (k : Nat) : Region := ⟨magBase (c.slotsOf' s k) (c.w s k), 8 * c.slotsOf' s k⟩

/-! ### The memory a routine may access -/

/-- What a routine may access whatever it finds, and whether it may write
it: `__rseq_offset` and `cpu_id` (read), `rseq_cs` (written), the masks'
table (read), and its own tables (read). -/
def fixed : List (Region × Bool) :=
  [(c.rsVar s, false), (c.cpuField s, false), (c.csField s, true), (c.maskTable s, false)] ++
  match c.kind with
  | .malloc => [(c.mallocTable s, false)]
  | .free => [(c.published s, false), (c.regionTable s, false)]
  | .freeSized => [(c.sizedTable s, false), (c.published s, false), (c.regionTable s, false)]
  | .calloc => [(c.callocTable s, false)]

/-- What it may touch if its CPU number has a slab and it finds a class:
the class's word in that slab, and the magazine the word names, if it names
one (both written). -/
def more : List (Region × Bool) :=
  if c.InRange s then
    match c.cls? s with
    | some k => (⟨c.word s k, 8⟩, true) ::
        (if IsSlot (c.slotsOf' s k) (c.w s k) then [(c.magRegion s k, true)] else [])
    | none => []
  else []

/-- **The memory a routine may access**, a function of the state, so
recomputed on a retry. -/
def regions : List (Region × Bool) := c.fixed s ++ c.more s

def ro : List Region := ((c.regions s).filter (!·.2)).map (·.1)
def rw : List Region := ((c.regions s).filter (·.2)).map (·.1)

/-- Where a magazine may be: not wrapping around, and apart from the slabs,
from what `fixed` names and from the reserved stack. -/
def Apart (r : Region) : Prop :=
  r.base.toNat + r.len ≤ 2 ^ c.A.ptrBits ∧ r.Disjoint (c.slabs s) ∧
    (∀ x ∈ c.fixed s, r.Disjoint x.1) ∧ ∀ q ∈ c.A.reserved 0 s, r.Disjoint q

/-- Where the slabs and what `fixed` names are, as `Spec.Slab.Ctx.Layout`
says it for the slab routines. -/
structure Layout : Prop where
  slabs_nowrap : (c.slabs s).base.toNat + (c.slabs s).len ≤ 2 ^ c.A.ptrBits
  slabs_res : ∀ q ∈ c.A.reserved 0 s, (c.slabs s).Disjoint q
  nowrap : ∀ x ∈ c.fixed s, x.1.base.toNat + x.1.len ≤ 2 ^ c.A.ptrBits
  disj : (c.fixed s).Pairwise (fun a b => (a.2 || b.2) → a.1.Disjoint b.1)
  slabs : ∀ x ∈ c.fixed s, x.1.Disjoint (c.slabs s)
  res : ∀ x ∈ c.fixed s, ∀ q ∈ c.A.reserved 0 s, x.1.Disjoint q

/-- The state's regions are the routine's, which do not wrap around,
overlap where one is written, or overlap the reserved stack. -/
structure World : Prop where
  rd : c.A.rd s = c.ro s
  wr : c.A.wr s = c.rw s
  nowrap : ∀ x ∈ c.regions s, x.1.base.toNat + x.1.len ≤ 2 ^ c.A.ptrBits
  disj : (c.regions s).Pairwise (fun a b => (a.2 || b.2) → a.1.Disjoint b.1)
  res : ∀ x ∈ c.regions s, ∀ r ∈ c.A.reserved 0 s, x.1.Disjoint r

/-- A word of the class `k`: zero, a marker, or a slot of a magazine of the
class's size that lies `Apart`. -/
def SlotOk (k : Nat) (w : BitVec 64) : Prop :=
  WordOk (c.slotsOf' s k) w ∧
    (IsSlot (c.slotsOf' s k) w → c.Apart s ⟨magBase (c.slotsOf' s k) w, 8 * c.slotsOf' s k⟩)

/-- Each slab's magazine of the class `k` is its own. -/
def Exclusive (k : Nat) : Prop := ∀ i < SLAB_COUNT, ∀ j < SLAB_COUNT, i ≠ j →
  IsSlot (c.slotsOf' s k) (c.wordOf s k i) → IsSlot (c.slotsOf' s k) (c.wordOf s k j) →
  Region.Disjoint ⟨magBase (c.slotsOf' s k) (c.wordOf s k i), 8 * c.slotsOf' s k⟩
    ⟨magBase (c.slotsOf' s k) (c.wordOf s k j), 8 * c.slotsOf' s k⟩

/-- **The invariant**, of every class byte: its mask is zero or an allowed
one, and if it is not zero every slab's word of the class is `SlotOk`; and
each slab's magazine of the class is its own: the pop's and push's
(`Spec.Slab.Ctx.inv`) for every class at once, with a mask that may be
zero. -/
def inv : Prop := ∀ k < MASK_COUNT,
  (c.maskOf s k = 0 ∨ MaskOk (c.maskOf s k)) ∧
  (c.maskOf s k ≠ 0 → ∀ i < SLAB_COUNT, c.SlotOk s k (c.wordOf s k i)) ∧ c.Exclusive s k

/-- The precondition beyond the signature's. The arguments may be anything. -/
def pre : Prop := c.A.tp.isSome ∧ c.A.sym.isSome ∧ c.Layout s ∧ c.inv s

/-! ### What a routine does -/

/-- The pointer a pop takes and what it leaves, if the CPU number has a
slab, the routine finds a class and its magazine has a pointer to give. -/
def popResult : Option (Nat × BitVec 64 × Mag) :=
  if c.InRange s then (c.cls? s).bind fun k => (c.mag s k).pop.map fun (x, m) => (k, x, m) else none

/-- A pop that took `x` from the class `k`: memory changed only in
`rseq_cs` and the slab word, which names the slot below, and the magazine is
the rest. -/
def Popped (s' : M.State) (x : BitVec 64) : Prop :=
  ∃ k rest, c.popResult s = some (k, x, rest) ∧
    Unchanged [c.csField s, ⟨c.word s k, 8⟩] (c.mem s) (c.A.mem s') ∧
    (c.A.mem s').readW (c.word s k) 64 = c.w s k - 8 ∧
    Mag.ofWord (c.slotsOf' s k) (c.A.mem s') (c.w s k - 8) = rest

/-- A pop that missed: memory changed only in `rseq_cs`. -/
def PopMissed (s' : M.State) : Prop :=
  c.popResult s = none ∧ Unchanged [c.csField s] (c.mem s) (c.A.mem s')

/-- The magazine a push of `x` leaves, if the CPU number has a slab, the
routine finds a class and its magazine has a free slot to spare. -/
def pushResult (x : BitVec 64) : Option (Nat × Mag) :=
  if c.InRange s then (c.cls? s).bind fun k => ((c.mag s k).push (c.slotsOf' s k) x).map (k, ·)
  else none

/-- A push of `x` to the class `k`: memory changed only in `rseq_cs`, the
slab word, which names the slot above, and the free slot it named. -/
def Pushed (x : BitVec 64) (s' : M.State) : Prop :=
  ∃ k rest, c.pushResult s x = some (k, rest) ∧
    Unchanged [c.csField s, ⟨c.word s k, 8⟩, ⟨c.w s k, 8⟩] (c.mem s) (c.A.mem s') ∧
    (c.A.mem s').readW (c.word s k) 64 = c.w s k + 8 ∧
    Mag.ofWord (c.slotsOf' s k) (c.A.mem s') (c.w s k + 8) = rest

/-- A push that missed: memory changed only in `rseq_cs`. -/
def PushMissed (x : BitVec 64) (s' : M.State) : Prop :=
  c.pushResult s x = none ∧ Unchanged [c.csField s] (c.mem s) (c.A.mem s')

/-- Where a push may be aborted: memory has changed only in `rseq_cs` and,
if it found a class, the free slot; and every slab's magazine of that class
is as it was. -/
def PushAbort (p : M.State) : Prop :=
  match (if c.InRange s then c.cls? s else none) with
  | none => Unchanged [c.csField s] (c.mem s) (c.A.mem p)
  | some k => Unchanged [c.csField s, ⟨c.w s k, 8⟩] (c.mem s) (c.A.mem p) ∧
      ∀ i < SLAB_COUNT, Mag.ofWord (c.slotsOf' s k) (c.A.mem p) (c.wordOf s k i) =
        Mag.ofWord (c.slotsOf' s k) (c.mem s) (c.wordOf s k i)

/-! ### Accesses

As the slab routines': `__rseq_offset` read (`o`), `rseq_cs` written
(`d`), `cpu_id` read (`i`); then, for a CPU number with a slab, the reads
that find the class, each of the values it read, and for the class `k`
found the slab word (`w`) and the mask (`m`); then the pop's slot read
(`v`) and commit, or the push's store of its pointer in the free slot and
commit. The commit is the last access and the only store to a slab word. -/

def rd (a : Addr) (n v : Nat) : Event := ⟨a, n, false, v⟩
def wr (a : Addr) (n v : Nat) : Event := ⟨a, n, true, v⟩

def head (o d i : Nat) : List Event :=
  let a := c.slab.tp s + BitVec.ofNat 64 o
  [rd (c.sym s RSEQ_OFFSET) 8 o, wr (a + BitVec.ofNat 64 RSEQ_CS) 8 d,
    rd (a + BitVec.ofNat 64 RSEQ_CPU_ID) 4 i]

/-- The accesses of a pop of the class `k` on the CPU `i`, given the word
`w`, the mask `m` and the slot `v` it read. -/
def popTail (i k w m v : Nat) : List Event :=
  [rd (wordAddr (c.sym s SLABS) i k) 8 w,
    rd (c.sym s SLABS + BitVec.ofNat 64 (8 * k + MASK_DISP)) 4 m] ++
  if BitVec.ofNat 32 w &&& BitVec.ofNat 32 m ≠ 0 then
    [rd (BitVec.ofNat 64 w - 8) 8 v, wr (wordAddr (c.sym s SLABS) i k) 8 (BitVec.ofNat 64 w - 8).toNat]
  else []

/-- The accesses of a push of `x` to the class `k` on the CPU `i`. -/
def pushTail (x : BitVec 64) (i k w m : Nat) : List Event :=
  [rd (wordAddr (c.sym s SLABS) i k) 8 w,
    rd (c.sym s SLABS + BitVec.ofNat 64 (8 * k + MASK_DISP)) 4 m] ++
  if BitVec.ofNat 32 (w + 8) &&& BitVec.ofNat 32 m ≠ 0 then
    [wr (BitVec.ofNat 64 w) 8 x.toNat, wr (wordAddr (c.sym s SLABS) i k) 8 (BitVec.ofNat 64 w + 8).toNat]
  else []

/-- The accesses that find the region's byte: the shift (`h`), the base
index (`b`), and, for an index in range, the byte (`r`). -/
def regionReads (h b r : Nat) : List Event :=
  let idx := (c.ptr s >>> (h % 64)) - BitVec.ofNat 64 b
  [rd (c.published s).base 4 h, rd ((c.published s).base + 8) 8 b] ++
  if idx.toNat < REGION_COUNT then [rd ((c.regionTable s).base + idx) 1 r] else []

/-- The accesses of `malloc`. -/
def MallocTrace (evs : List Event) : Prop :=
  ∃ o d i k w m v : Nat, evs = c.head s o d i ++
    if i < SLAB_COUNT ∧ (c.size s).toNat ≤ MALLOC_MAX then
      rd ((c.mallocTable s).base + BitVec.ofNat 64 (((c.size s).toNat + 7) / 8)) 1 k ::
        c.popTail s i k w m v
    else []

/-- The accesses of `free`. -/
def FreeTrace (evs : List Event) : Prop :=
  ∃ o d i h b r w m : Nat, evs = c.head s o d i ++
    if i < SLAB_COUNT ∧ (c.ptr s).toNat ≠ 0 then
      c.regionReads s h b r ++
        if ((c.ptr s >>> (h % 64)) - BitVec.ofNat 64 b).toNat < REGION_COUNT then
          c.pushTail s (c.ptr s) i r w m else []
    else []

/-- The accesses of the sized free: the predicted class (`k`) first. -/
def FreeSizedTrace (evs : List Event) : Prop :=
  ∃ o d i k h b r w m : Nat, evs = c.head s o d i ++
    if i < SLAB_COUNT then
      rd ((c.sizedTable s).base + BitVec.ofNat 64 ((c.size s).toNat % SIZED_LEN)) 1 k ::
        c.regionReads s h b r ++
        if ((c.ptr s >>> (h % 64)) - BitVec.ofNat 64 b).toNat < REGION_COUNT ∧ r = k then
          c.pushTail s (c.ptr s) i k w m else []
    else []

/-- The accesses of `calloc`: none to find the product, then the class (`k`) for a total its
table covers. -/
def CallocTrace (evs : List Event) : Prop :=
  ∃ o d i k w m v : Nat, evs = c.head s o d i ++
    if i < SLAB_COUNT ∧ ¬ c.overflows s ∧ (c.size s).toNat ≤ CALLOC_MAX then
      rd ((c.callocTable s).base + BitVec.ofNat 64 (c.size s).toNat) 1 k :: c.popTail s i k w m v
    else []

/-- The third argument register, where `calloc` hands its zeroing tail the block. -/
def third : BitVec 64 := match c.A.args [c.A.ptrBits, c.A.ptrBits, c.A.ptrBits] with
  | some f => (f s).getD 2 0
  | none => 0

end Ctx

/-! ## The contracts -/

/-- The contract of a C routine, as `Spec.Slab.contract` builds the slab
routines': its signature's, its memory `Ctx.regions`, the precondition
`Ctx.pre`, and, where it may be aborted, the invariant kept. Other threads,
the drainer and the operating system may change any memory but
`__rseq_offset` and the return address's slot while it is aborted, keeping
the invariant. -/
def contract {M : ISA} (c : Ctx M) (post : M.State → M.State → Prop)
    (exit : String → M.State → M.State → Prop) (abort : M.State → M.State → Prop)
    (trace : M.State → List Event → Prop) : Contract M :=
  let A := c.A.withExtra (some c.regions)
  let x : Sig.Extra M := {
    pre := c.pre
    post := post
    exit := exit
    abort := fun s p => abort s p ∧ c.inv p
    trace := trace
    env := fun p p' => c.A.rd p' = c.ro p' ∧ c.A.wr p' = c.rw p' ∧
      Kept (c.rsVar p :: c.A.reserved 0 p) (c.mem p) (c.mem p') ∧ (c.inv p → c.inv p') }
  c.sig.contract A (post := Curry.const (fun _ _ _ => True) _) (x := x)

/-- `c_malloc`: `fn(size: usize) -> usize`. -/
def mallocSig : Sig where
  params := [("size", .int .usize)]
  ret := some .usize

/-- `c_free`: `fn(ptr: usize)`. -/
def freeSig : Sig where
  params := [("ptr", .int .usize)]

/-- `c_free_sized`: `fn(ptr: usize, size: usize)`. -/
def freeSizedSig : Sig where
  params := [("ptr", .int .usize), ("size", .int .usize)]

/-- `c_calloc`: `fn(n: usize, size: usize) -> usize`; it never returns itself, leaving for its
slow path or its zeroing tail, which return the block. -/
def callocSig : Sig where
  params := [("n", .int .usize), ("size", .int .usize)]
  ret := some .usize

/-- `calloc`: if `n * size` does not overflow and is at most `CALLOC_MAX`, pops a pointer of
the class its table gives the total and tail-calls the zeroing tail with `n`, `size` and the
pointer (in the third argument's register); otherwise, or if there is nothing to pop,
tail-calls the slow path with `n` and `size`. -/
def callocContract {M : ISA} (A : Abi M) : Contract M :=
  let c : Ctx M := ⟨A, .calloc, callocSig, .c⟩
  contract c
    (post := fun _ _ => False)
    (exit := fun t s s' => (t = SLOW ∧ c.PopMissed s s') ∨ (t = ZERO ∧ ∃ x, c.Popped s s' x ∧ c.third s' = x))
    (abort := fun s p => Unchanged [c.csField s] (c.mem s) (A.mem p))
    (trace := c.CallocTrace)

/-- `malloc`: pops a pointer of the class its table gives the size and
returns it, or tail-calls the slow path with the size. -/
def mallocContract {M : ISA} (A : Abi M) : Contract M :=
  let c : Ctx M := ⟨A, .malloc, mallocSig, .c⟩
  contract c
    (post := fun s s' => ∃ x, c.Popped s s' x ∧ A.ret s' = x)
    (exit := fun t s s' => t = SLOW ∧ c.PopMissed s s')
    (abort := fun s p => Unchanged [c.csField s] (c.mem s) (A.mem p))
    (trace := c.MallocTrace)

/-- `free`: returns at once for null; pushes the pointer to the class of its
region; or tail-calls the slow path with the pointer. -/
def freeContract {M : ISA} (A : Abi M) (m : Mirror) : Contract M :=
  let c : Ctx M := ⟨A, .free, freeSig, m⟩
  contract c
    (post := fun s s' => ((c.ptr s).toNat = 0 ∧ Unchanged [c.csField s] (c.mem s) (A.mem s')) ∨
      c.Pushed s (c.ptr s) s')
    (exit := fun t s s' => t = SLOW ∧ c.PushMissed s (c.ptr s) s')
    (abort := c.PushAbort)
    (trace := c.FreeTrace)

/-- The sized free: pushes the pointer to the class its size predicts if
that is the class of its region, or tail-calls the slow path with the
pointer and the size. -/
def freeSizedContract {M : ISA} (A : Abi M) (m : Mirror) : Contract M :=
  let c : Ctx M := ⟨A, .freeSized, freeSizedSig, m⟩
  contract c
    (post := fun s s' => c.Pushed s (c.ptr s) s')
    (exit := fun t s s' => t = SLOW ∧ c.PushMissed s (c.ptr s) s')
    (abort := c.PushAbort)
    (trace := c.FreeSizedTrace)

/-! ## The functions -/

/-- The Rust paths of what the code names. -/
def slowPath (n : String) : String := "crate::c::slow::" ++ n
def tablePath (n : String) : String := "crate::c::table::" ++ n

/-- What every C routine requires: of the thread, of where things are, and
of the invariant. -/
def safety (tables : List String) : List String :=
  [registeredDoc,
  s!"None of `SLABS` (the slabs, `{SLAB_COUNT} * {SLAB_BYTES}` bytes), the thread's `rseq_cs` \
    and `cpu_id`, `__rseq_offset`, the masks' table (2048 bytes {MASK_DISP} bytes above \
    `SLABS`), {englishList "and" tables} may overlap the return address on the stack or wrap \
    around the end of the address space, and `rseq_cs` must not overlap any of the others; \
    nor may any but the slabs overlap the slabs (`VG.Spec.CEntry.Ctx.Layout`). Their contents \
    may be anything but as the next item says.",
  s!"For every class byte `k` below {MASK_COUNT}, the low 32 bits of the masks' table's entry \
    `k` must be 0 or the mask `8 * n - 16` of magazines of `n = 2^j` slots for \
    {MAG_LOG_MIN} ≤ j ≤ {MAG_LOG_MAX}; if it is not 0, in each of the {SLAB_COUNT} slabs the \
    word `8 * k` must be 0, a marker (a word whose low {MARKER_SHIFT} bits are 1), or the \
    address of a slot other than slot 0 of a magazine of that size (`8 * n` bytes, aligned to \
    `8 * n`) that overlaps none of the memory the previous item names, the return address on \
    the stack, or another slab's magazine of the class, nor wraps around the end of the \
    address space. Every thread and the drainer must keep this from when the function is \
    called until it returns: the function may be aborted and restarted on any CPU in between \
    (`VG.Spec.CEntry.Ctx.inv`)."]

/-- The tables `free` reads, as the documentation names them. -/
def regionDoc (m : Mirror) : List String :=
  [s!"the published shift and base index (16 bytes {m.disp} bytes above `SLABS`)",
    s!"the region table ({REGION_COUNT} bytes {REGION_DISP} bytes above `SLABS`)"]

def operands (table : Option String) (slow : String) : List (String × Operand) :=
  [(RSEQ_OFFSET, .sym (path RSEQ_OFFSET)), (SLABS, .sym (path SLABS))] ++
  (table.map fun t => (t, Operand.sym (tablePath t))).toList ++
  [(SLOW, .sym slow)]

/-- The layout constants the Rust side asserts. -/
def rustConsts : List (String × IntTy × Nat) :=
  [("SHIFT_DISP", .usize, SHIFT_DISP), ("BASE_DISP", .usize, BASE_DISP),
    ("MIRROR_BYTES", .usize, MIRROR_BYTES), ("MIRROR_COUNT", .usize, MIRROR_COUNT),
    ("REGION_DISP", .usize, REGION_DISP), ("REGION_COUNT", .usize, REGION_COUNT),
    ("TABLE_OFF", .usize, TABLE_OFF), ("MALLOC_MAX", .usize, MALLOC_MAX),
    ("MALLOC_LEN", .usize, MALLOC_LEN), ("SIZED_LEN", .usize, SIZED_LEN),
    ("MASK_DISP", .usize, MASK_DISP), ("MASK_COUNT", .usize, MASK_COUNT),
    ("SLAB_COUNT", .usize, SLAB_COUNT), ("SLAB_BYTES", .usize, SLAB_BYTES)]

def mallocApi : Api where
  module := "centry"
  name := "c_malloc"
  sig := mallocSig
  contracts := some fun A _ => mallocContract A
  summary := "C's `malloc`: pops a block of the class `C_SIZES[(size + 7) / 8]` from the \
    calling thread's CPU's magazine and returns it, or tail-calls `c::slow::malloc_slow(size)` \
    if the size is above 32768, the CPU number has no slab, the class's mask is 0 or its \
    magazine has none to give. `malloc_slow` starts with the thread's `rseq_cs` still pointing \
    at this function's sequence, which it has left.\n\n\
    Contract: `VG.Spec.CEntry.mallocContract`."
  safety := safety [s!"the {MALLOC_LEN} bytes {TABLE_OFF} bytes into `C_SIZES`"]
  operands := operands (some SIZES) (slowPath "malloc_slow")
  rustConsts := rustConsts
  exports := [("c", "malloc")]

/-- A free's `Api`: its name, mirror, slow path and the symbols it is exported as. -/
def freeApiOf (name : String) (m : Mirror) (slow : String) (exports : List (String × String)) :
    Api where
  module := "centry"
  name := name
  sig := freeSig
  contracts := some fun A _ => freeContract A m
  summary := s!"For {m.doc}: returns at once for null; otherwise pushes `ptr` on the calling \
    thread's CPU's magazine of the class its region's byte names, the region index being \
    `(ptr >> shift) - base_index` from the words of the mirror at {m.disp} bytes above `SLABS`, or tail-calls \
    `{slow}(ptr)` if the index is not below 4096, the CPU number has no slab, the \
    class's mask is 0 or its magazine has no free slot to spare.\n\n\
    Contract: `VG.Spec.CEntry.freeContract`."
  safety := safety (regionDoc m)
  operands := operands none slow
  rustConsts := rustConsts
  exports := exports

/-- A sized free's `Api`. -/
def freeSizedApiOf (name : String) (m : Mirror) (slow : String)
    (exports : List (String × String)) : Api where
  module := "centry"
  name := name
  sig := freeSizedSig
  contracts := some fun A _ => freeSizedContract A m
  summary := s!"For {m.doc}: the class `C_SIZED_SIZES[size % 65536]` is a prediction, which the class \
    byte of `ptr`'s region must match, the region found as the plain free with the same \
    mirror finds it; then pushes `ptr` on the calling thread's CPU's magazine of \
    that class, or tail-calls `{slow}(ptr, size)` if the two disagree, the region index is not \
    below 4096, the CPU number has no slab, the class's mask is 0 or its magazine has no free \
    slot to spare. A wrong size frees exactly what the plain free would.\n\n\
    Contract: `VG.Spec.CEntry.freeSizedContract`."
  safety := safety (s!"the {SIZED_LEN} bytes {TABLE_OFF} bytes into `C_SIZED_SIZES`" :: regionDoc m)
  operands := operands (some SIZED) slow
  rustConsts := rustConsts
  exports := exports

/-- C's `free` and `free_sized`. -/
def freeApi : Api := freeApiOf "c_free" .c (slowPath "free_slow") [("c", "free")]
def freeSizedApi : Api :=
  freeSizedApiOf "c_free_sized" .c (slowPath "free_sized_slow") [("c", "free_sized")]

def callocApi : Api where
  module := "centry"
  name := "c_calloc"
  sig := callocSig
  contracts := some fun A _ => callocContract A
  summary := "C's `calloc`: for a product `n * size` that does not overflow and is at most 1024, \
    pops a block of the class `C_CALLOC_SIZES[n * size]` from the calling thread's CPU's \
    magazine and tail-calls `c::slow::calloc_zero_tail(n, size, block)`, which zeroes it and \
    returns it; otherwise, or if the CPU number has no slab, the class's mask is 0 or its \
    magazine has none to give, tail-calls `c::slow::calloc_slow_args(n, size)`.\n\n\
    Contract: `VG.Spec.CEntry.callocContract`."
  safety := safety [s!"the {CALLOC_LEN} bytes {TABLE_OFF} bytes into `C_CALLOC_SIZES`"]
  operands := operands (some CALLOC_SIZES) (slowPath "calloc_slow_args") ++
    [(ZERO, .sym (slowPath "calloc_zero_tail"))]
  rustConsts := rustConsts ++ [("CALLOC_MAX", .usize, CALLOC_MAX), ("CALLOC_LEN", .usize, CALLOC_LEN)]
  exports := [("c", "calloc")]

end VG.Spec.CEntry
