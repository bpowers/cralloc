# Safety ledger: `crates/asmalloc/src/front/twin.rs`

The twins implement the routines' specifications in Rust. Their unsafe blocks turn a slab word
or a spare-stack head into an atomic view of a magazine slot. Kani checks the twins' behaviour
against the specifications on a small slab (`front/verify.rs`); the obligation here is that the
addresses they dereference are valid, which rests on invariants the slow paths and the drain
keep, not on anything the twin can check.

**The invariant (S).** For every CPU with a slab and every small class:

- the hot word is zero, a marker (`classes::is_marker`), or the address of slot `s`, with
  `1 <= s <= n - 1`, of a magazine of the class's size `n` that lies in committed magazine
  memory; and
- each spare head, and the link (slot 0) of each magazine on a spare stack, is zero or the slot-0
  address of a magazine of the class's size in committed magazine memory. Never a marker below
  the head: `stack_push` refuses to push onto a marker, so a link is always the head it saw,
  which was zero or a magazine.

Committed magazine memory stays mapped and writable for the life of the reservation (the `Os`
contract; nothing in the crate purges or unmaps the magazine area), and every access to a
magazine slot anywhere in the crate is atomic.

**Why (S) holds.** Outside the crate, nothing can write a slab field: `Slab::field` is
crate-private, and the only public ways in are the routines. Of those, pop and push keep (S)
by themselves (below); store-if-equal and stack push are `unsafe fn`s whose contracts require
exactly the values (S) allows, so a caller that breaks (S) has broken an unsafe contract. Writers
of slab fields are:

- the twins' (and routines') commits: pop stores `w - 8` only when `w & mask != 0`, so `s >= 2`
  and the new slot is `s - 1 >= 1`; push stores `w + 8` only when `(w + 8) & mask != 0`, so
  `s <= n - 2` and the new slot is at most `n - 1`. `classes::verify` proves both tests for every
  size, address and marker. The stack routines store a magazine address the slow path holds
  (below) or the link they read, which (S) covers;
- store-if-equal, whose contract requires `new` to keep (S). The crate's only callers (SLOW-01)
  pass `Magazine::word(level)` with `level <= capacity`, so slot `level + 1` in `1..=n - 1` of
  a `Magazine` handle;
- the drain, which stores its marker, zero, or (in back-out) a value it exchanged out of the same
  field, with markers mapped to zero.

Spare-stack links (slot 0 of a magazine on a spare stack) are written only by the stack push
that put the magazine there, and by whoever holds the magazine's handle: the depot stacks write
slot 0 through `Magazine::link`, which is crate-private, and `Magazine::set_object` refuses slot 0
and the last slot. A handle to a magazine on a stack cannot be had outside the crate: the lookups
that make a handle from an address or a number (`Area::magazine`, `by_id`, `loaded`) are
crate-private, and the public ways to get one, `Area::create` and a stack pop, give a magazine
nothing else refers to; handles are neither `Clone` nor `Copy`. Inside the crate, a handle to a
magazine is made only by the thread that took it off a stack or out of a slab word, which it then
owns (SLOW-01, SPARE-01, the drain's step 4).

A `Magazine` handle exists only for an offset `magazine::admits` accepted: past the generation
page, aligned to its size and inside the committed bytes (`depot::verify::
an_admitted_magazine_lies_inside_the_committed_area`). The magazine area itself is aligned to
at least the largest magazine (`Geometry::magazine_area_align`), so an aligned offset is an
aligned address. So every magazine address the slow paths give a routine is in committed memory,
and so is every value derived from one.

## TWIN-01: `slot(top)` in `try_pop` and `slot(word)` in `try_push`

**Preconditions of `slot`.** The address is a magazine slot in committed magazine memory, and
every access to it is atomic (the conditions of `AtomicPtr::from_ptr`: aligned to 8, valid for
reads and writes for the reference's lifetime, no non-atomic access racing).

**Proof.** In `try_pop`, `word` passed `!alloc_slow(word)`, so it is neither zero nor a marker,
and by (S) it is slot `s >= 2` of a committed magazine; `top = word - 8` is slot `s - 1 >= 1`
of the same magazine. In `try_push`, `word` passed `!free_slow(word)`, so by (S) it is slot
`s` with `1 <= s <= n - 2`. Both are inside the magazine, 8-aligned because the magazine is
aligned to its size (at least 32 bytes). The memory stays mapped, so the `'a` lifetime the
caller picks is not outlived. All accesses to magazine slots in the crate go through atomic
views (`Magazine::slot`, `slot` here, the stack links), so nothing races non-atomically.

The pointer carries the provenance of the word it came from, which the slow path built from the
reservation's base pointer (`Area::at`), so it is valid for the whole reservation.

## TWIN-02: `slot(magazine)` in `try_stack_push`

**Proof.** `try_stack_push` is reached only from `stack_push`, an `unsafe fn` whose contract
says `magazine` is the slot-0 address of a magazine of the field's class's size, in committed
magazine memory, that the caller holds. Slot 0 is inside it and 8-aligned. The crate's only
caller (SPARE-01) passes `Magazine::base` of a handle it holds.

## TWIN-03: `slot(head)` in `try_stack_pop`

**Proof.** `head` is not zero and not a marker, so by (S) it is the slot-0 address of a
committed magazine.

## TWIN-04: `unsafe impl Routines for Twin`

**Obligations** (the trait's `# Safety`). Each method behaves as its specification, as one step
that either commits or changes nothing, on the slab of the CPU the thread was on throughout.

**Proof.** Each `try_*` follows its specification in `routines.rs` line for line, and Kani
checks the outcomes (`front::verify`). Before every step after the first it calls
`Machine::step`, and an `Err` returns before any store, so an attempt that does not reach its
commit has stored at most to a magazine slot at or above the hot word (push) or to the link of
a magazine only the caller holds (stack push), which (S) and the specification leave unobserved.
The commit is the attempt's last store. The CPU is read once per attempt, in the first step, and
the machine's contract is that any change of CPU aborts the attempt, so every access of an
attempt that commits is to that CPU's slab. Tests abort every routine before each of its steps,
with and without a migration and a nested call, and check that nothing visible changed
(`front::twin::tests`).

**Not covered.** Whether the simulated machine keeps its contract (aborting on a change of CPU,
and a fence aborting sequences in flight) is a property of the test machines, not of this file.
