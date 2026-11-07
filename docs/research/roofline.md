# What gates the fast path

An allocate and free inlined into Rust costs cralloc 5.0 cycles at a constant size and 7.3 at a size
known only at run time (5.5 and 7.5 in the form a shared library can use). The floor for a per-CPU
magazine under restartable sequences (rseq) is 4.0; for any magazine, 2.0. Incumbents through the
same interface cost mostly 10 to 15 with fat link-time optimization (LTO). Figures are per
allocate-and-free unless they say "a call", and from one machine (`hardware.md`).

What sets the cycles is how each access gets its data. Ranked roughly by what one event costs (a
reload is per hop, a late store per pair of a mixed loop):

| Access pattern | Cost on Zen 5 | Rule | Enforced by |
|---|---|---|---|
| A cache line that moves between cores | 90 to 125 cycles in one L3, about 440 across L3s | No shared line per operation: per-CPU slabs | Construction; Lean proves what each routine touches |
| A write into freed memory | 40 to 45 cycles in one L3, about 150 across, paid by the object's next user | A free writes only the CPU's own magazine and slab word | The same proof |
| An atomic | 20 cycles uncontended, four or five inlined pairs | None on the fast path; the commit is a plain store | Lean: the model has no locked instruction, and the emitter requires a plain-store commit |
| A load at the page offset of a store in flight | A loop goes from 1 cycle a step to 2, 10 or 18 to 25; one job ran 23, 31 or 58 cycles an object | Nothing a fast path loads on every call overlaps a hot word (the slab word a call stores) in the low 12 bits, with four listed limits | Compile-time asserts; a test and a script on the linked binary; the benchmark runner refuses a collision |
| A reload that loses memory renaming: a `movzx`, an absolute address, a hop across `call [mem]` | 5 to 7 cycles a hop, against 0 | Hot state is 64 bits, through a base register. A caller's `call [mem]` is out of our hands | Lean, for our routines: every memory operand in the model has a base register, and a routine's contract is a 64-bit word |
| A store whose address arrives late | About 12 cycles a pair in a loop of mixed sizes | Take the class from the size, not through the pointer | Rust's `dealloc` by construction; C's sized free uses the size as a prediction |
| A straight run past the front end's windows | 1 cycle a call; 7 to 9 when the next load is waiting on memory | Out of line: at most 24 fused micro-ops and 128 bytes, not 65 to 75, entry on a line. Inlined: at most 13, which keeps the routine small and does not remove this cost, since the run is the caller's | Out of line: Lean's `checkUops` and `lean/scripts/check_sizes.py` on the assembled bytes. Inlined: `checkUops` for the cap, nothing for the caller's run |
| Reaching the path through a call | 4 cycles a call; the PLT 1 more; an empty marker call 5 to 6.5 a pair | Inline under LTO | A check run by hand (`bench/roofline/fastpath`) |

The design removes the first three from every fast path; slow paths still pay atomics. The rules
remove the next two, apart from the limits listed under each and callers that use `call [mem]`.
Three things still set the cycles:

- **In a tight loop, op count and the chain to the slab word.** A constant-size pair is 33.7 ops. At
  8 a cycle, retirement alone would take 4.2 of its 4.96 cycles, and nothing yet separates that from
  the front end. Fewer ops help only until the loads, which wait on the chain through the CPU
  number, take over: taking out 5 ops saved 0.22 cycles.
- **With mixed sizes and cold objects, by inference, loads.** Instructions a cycle fall from 8 to
  5.9 on mixed sizes and to 3.8 on churn over 10,000 objects.
- **In real programs, misses and where the caller's code lands.** A pair costs about 14 cycles on
  servers against 5 to 7 in a loop, with the core shared in the first case and not the second ("Why
  a pair costs more in real programs"). The fast path is 11% of the log store's cycles, where it is
  largest, and 1 to 5% elsewhere.

Byte alignment by itself, byte savings and where the store and reload sit in the code do not matter,
and a routine's position matters only through lines ("What turned out not to matter").

## The floor and where cralloc stands

A roofline is the cost of the cheapest code that could do a job, measured, so that an allocator can
be compared with a floor and not only with other allocators. A magazine is an array of object
pointers used as a stack. A fused micro-op is one instruction, with a `cmp` or `test` and the jump
after it counted together. A pair allocates one object and frees it at once, the tightest dependency
chain an allocator has. A batch is 32 allocations and then 32 frees.

Hand-written assembly, cycles per allocate and free (2026-10-16; pinned, SMT sibling idle, fastest
of three runs of 40 million iterations, from user-mode counters, which do not depend on the clock;
`bench/roofline/README.md` has the harness):

| Shape | Pair | Through two calls | Batch |
|---|---:|---:|---:|
| Bare magazine, everything hoisted: two dependent operations | 2.00 | | |
| Thread-local list, through a base register | 2.28 | 8.00 | 4.7 |
| Thread-local magazine, through a base register | 2.36 | 8.00 | 3.0 |
| Per-CPU list under rseq | 2.85 | 8.00 | 5.1 |
| Per-CPU magazine under rseq | 3.93 to 4.02 | 8.00 | 4.7 to 5.5 |
| tcmalloc's `Pop` and `Push`, instruction for instruction | 4.82 | 8.00 | 5.8 |
| Thread-local list, head at `fs:[constant]` | 7.0 to 7.2 | 8.00 | 8.7 to 8.9 |
| Thread-local magazine, top at `fs:[constant]` | 15.9 | 15.9 | 8.3 to 11.5 |

- **rseq costs about 1.6 cycles inlined and nothing through a call.** Arming the section (a `lea`
  and a store) and finding the slab (a load, a shift, an add) each take the loop from 2.00 to 4.00
  alone, and both together still cost 4.00. The cause is micro-ops, not either step.
- **An array beats a list in bursts by 1.7 cycles** even with everything in L1: popping a list is a
  pointer chase of 4 cycles a load, and popping an array is an add. A list also makes misses on cold
  objects serial; that was not measured.

**Rust, through `GlobalAlloc`, fat LTO.** Churn is 10,000 live objects of 16 to 512 bytes, one freed
and one allocated per step at random. cralloc is 2026-10-22 at `198fc46`: median of three rounds at
each of four code placements, then the mean; placements agree within 0.3 cycles except where a range
is shown. The executable form reads `__rseq_offset` with one load, the default form with two. The
rest is 2026-10-17, same harness, second of two interleaved rounds. The floor is a list behind a
thread-local pointer; it never returns memory and serves only small sizes.

| | Pair, constant | Pair, `Box` | Pair, run-time size | Batch of 32 | Churn |
|---|---:|---:|---:|---:|---:|
| Floor | 2.2 | 4.0 | 3.7 | 8.8 | 16.4 |
| cralloc, executable form | 4.96 | 5.90 | 7.26 | 5.36 | 23.9 |
| cralloc, default form | 5.4 to 6.0 | 6.95 | 7.52 | 6.0 to 6.4 | 24.5 |
| mimalloc, sized free | 10.2 | 10.3 | 10.2 | 9.6 | 30.3 |
| mimalloc | 12.3 | 13.6 | 12.9 | 10.7 | 37.4 |
| jemalloc | 12.1 | 12.1 | 14.8 | 11.9 | 39.7 |
| glibc | 13.0 | 11.0 | 21.9 | 32.4 | 46.9 |

Without LTO, cralloc's pairs cost 11 to 14 and the incumbents' 14 to 26. The incumbents execute 57
to 95 instructions a pair; the floors, 12 to 28; cralloc, 40 to 56. The `Box` column adds the
caller's initializing of 32 bytes. `standings.md` has the same allocators on whole jobs.

**C, through its entry points**, cycles per `malloc` and `free` from plain C loops (2026-10-17 and
19, best of three interleaved rounds, the library as shipped). Mixed cycles through 64 sizes from 1
to 512 bytes:

| | Pair 32 B | Burst of 32 | Burst of 100 | Mixed | `call [GOT]` pair |
|---|---:|---:|---:|---:|---:|
| cralloc, linked statically (direct call) | 8.03 | 8.7 | 9.3 | 22.6 | |
| cralloc, through the PLT | 10.00 | 10.4 | 10.8 | 23.7 | 28.2 |
| glibc | 12.00 | 37.1 | 47.8 | 14.2 | 22.4 |
| mimalloc | 12.2 | 11.5 | 12.3 | 19.0 | 16.1 |
| jemalloc | 14.1 | 13.2 to 13.4 | 14.1 to 14.2 | 31.6 | 39.7 |

Per-CPU tcmalloc runs 14.0 to 15.2 in the PLT cells and over 30 in mixed. So cralloc is at the floor
of two calls wherever the call decides, and behind where the pointer chain decides.

**Threads freeing each other's objects**, nanoseconds per object with both sides working (fat LTO,
bursts of 32, one L3; 2026-10-17). In a pipeline one thread allocates and another frees. In an
exchange each allocates for the other and frees what it receives. The floor keeps what it is given
on a local list, a fair bound on the exchange and none on the pipeline.

| | One thread | Pipeline | Exchange of 2 | Ring of 4 |
|---|---:|---:|---:|---:|
| Floor | 0.9 | | 2.7 | 3.0 |
| cralloc | 1.3 | 2.8 | 3.8 | 4.6 |
| mimalloc | 2.8 | 10.0 | 22.1 | 17.0 |
| jemalloc | 3.1 | 11.9 | 10.7 | 11.9 |
| glibc | 7.9 | 152 | 131 | 136 |

cralloc's rounds agree within 7%; mimalloc's pipelines moved by up to 60%. Within one L3, mimalloc
costs 1.5 to 2 times jemalloc on the exchange. Across L3s it costs 2.1 to 2.7 times on the pipeline
and about 1.8 times on the exchange (32 ns against 17; floor 9). By inference, mimalloc returns each
object to its page, with a locked instruction per free and more line movement, while jemalloc keeps
received objects in the freeing thread's cache, so it moves fewer lines at twice the instructions;
across L3s, where a line costs four times as much, line movement decides the ranking. These loops
use one size and two to four threads: they show a mechanism, not a server.

## The patterns, most expensive first

### A cache line that moves costs tens of fast paths

A line handed to another core costs 16 to 22 ns inside one L3 and 76 to 79 ns across L3s. An atomic
read-modify-write costs 20.0 cycles uncontended, whichever instruction it is. It serializes with
other atomics and overlaps unrelated work: two `lock add`s on different lines cost 40.0, and a `lock
add` beside 16 dependent adds costs 20.0, not 36. A lock-free stack pop and push costs 41.0.

**Rule.** No atomic and no shared cache line per operation. Atomics are amortized: two per magazine
exchanged in the depot, about one per 64 objects in the back end (`docs/design.md`, "Summary").
**Enforced.** The instruction model the routines are written in has no locked instruction, so none
can be written. The emitter refuses a restartable sequence that does not end in a plain store
(`Rust.checkRseq`). For each fast-path routine Lean proves what it may touch: it writes only
`rseq_cs`, one slab word and the magazine that word names, and otherwise reads only `__rseq_offset`,
`cpu_id` and, in C, tables the fast path never writes.

### A write into freed memory is paid by the object's next user

A producer reusing a 64-byte object that a consumer on another core read and wrote first pays 7 to 8
ns more in one L3, 27 ns across L3s, and 0.8 to 0.9 ns on an SMT sibling. An intrusive free list
writes into every object it frees.

**Rule.** Metadata never lives in freed memory. A free stores a pointer in the CPU's own magazine
and commits its slab word; it writes neither the object nor anything shared. **Enforced** by the
same footprint proof. (Both sections: 2026-10-16, single runs in two sessions; `hardware.md` has the
rest.)

### A load at the page offset of a store in flight can stall the whole loop

**Mechanism.** Zen 5 holds a load whose bytes overlap, in the low 12 bits of the address, a store
still in flight. On a fast path, the hot word (the slab word that holds the next free slot) is the
last thing a call stores, and the next call loads a table entry or `__rseq_offset` straight after.
Since the hot word's address comes from that load, the calls then run one at a time. Whether a load
is held, and for how long, is predictor state, so the same file runs differently from one run to the
next.

**The mechanism alone.** A loop loads a class byte, forms a slab word's address from it, loads the
word and stores it back, in 64 pairs of fresh pages. With the byte at -32 to -1 or +8 to +64 from
the word's page offset, it runs 1.00 cycle an iteration in all 64. At 0 to +7, overlapping the
store, it runs 2 in 17 or 18 pairs, 10 in 40 or 41 and 18 to 25 in 5. The mode changes in streaks
over a run and does not follow the page pair.

**In a job.** Allocating, writing and freeing 400 thousand 64-byte objects over and over ran 23, 31
or 58 cycles an object from one run of a file to the next, with equal instructions and cache and TLB
misses. The class table's 64-byte entry sat at that class's hot word's page offset. Op sampling put
the table load at 32 cycles from dispatch to completion in a fast run and 146 in a slow one, and the
loads that take their address from it at about 40 against 152. Code placement sweeps do not move a
table, so they could not show it.

| Cycles an object over four code layouts, a fresh copy of the binary per run | Before | After |
|---|---:|---:|
| 64-byte objects | 22.9 to 56.4 | 21.7 to 23.0 |
| 32-byte objects (the `__rseq_offset` copy on a hot word in one build) | 21.0 to 33.6 | 21.0 to 21.4 |
| 1,792-byte objects, the build that collided | 74.7, 83.6 | 66.6, 66.7 |

**Rule.** Nothing a fast path reads or writes on every call shares a page offset with a hot word.

- **Hot words sit at bytes 24 to 31 of a 32-byte chunk,** so that no 16-aligned 8-byte word can
  overlap one, and neither can the thread's `cpu_id` and `rseq_cs`, at bytes 4 to 15, since the
  kernel requires the rseq area to be 32-byte aligned. The move cost nothing: within 0.01 cycles at
  every placement in five forms and 0.13 and 0.45 in two.
- **The crate's tables start past every slab field:** the class bytes 2048 bytes into a page-aligned
  static, slot sizes and masks at page offsets 2048 to 2687 of its second page, the C routines'
  tables 2048 bytes into theirs. Beside the slabs, the masks and the region table start at page
  offset 2048, and the published heap words at 1024, bytes 0 to 15 of a chunk. Two earlier readings
  point the same way, by inference: a mask exactly 4096 bytes from its word cost an inlined pair 0.8
  cycles, and heap words at page offsets 8 and 16 cost a C pair 1.1. Out of line, a table 4096 bytes
  away measured the same as one that cannot alias.
- **The executable's copy of `__rseq_offset`** is 16-aligned whenever glibc's own is, since lld
  aligns a copy as the shared object aligns the symbol. Before hot words moved, it collided in four
  of eight builds.
- **A routine's own word replaces the GOT slot,** which lands on a hot word for one link in four.
  Each routine reads `__rseq_offset`'s address from a 16-aligned word of its own, 16 bytes an
  inlined copy. No proof changed, every routine kept its size, and C loops are within 0.12 cycles
  before and after.

**Enforced.** `front` asserts all of this at compile time for what the crate places, and a test
checks every table entry's address. For what the linker and glibc place, a test reads the test
binary's relocations, and `scripts/rseq-offset-check` does the same for any binary. It runs in
`scripts/preload-check` and in `benchrun build` and `run`, which refuse a collision and record the
offsets.

**Limits.** Each of these can still put a load at a hot word's page offset:

- a glibc whose `__rseq_offset` is only 8-aligned: its copy then lands on a hot word for about one
  link in twelve, and the checks report it;
- the size tables above 16 KiB, which wrap round the page: a loop of allocations of one size of 16
  to 32 KiB whose entry overlaps its own class's word;
- the sized free's table for sizes of 2 KiB and up, indexed by byte: about one size in twelve
  overlaps a hot word, and a loop of sized frees of such a size can serialize;
- the region table's top half: frees of objects in regions numbered 2048 and up.

**Held, not built:** a word of the crate's own for the rseq offset would put every load under the
rule. Zero must then mean "not yet", which needs a guarded sequence in the trusted core, 4 to 5
days, and would take `c_free_sized` past its 128 bytes. (Evidence: 2026-10-19 at `8b2c44d` and on
top of it, 2-second runs, two to five a cell, interleaved, each run copying the binary to a new file
so that its page-cache pages are new. A suite run of main against the move tied on every job but
one, which tied over twenty more rounds.)

### A reload that loses memory renaming costs 5 to 7 cycles

A fast path stores its state and the next call loads it back. Each cell is the loop `load; add 1;
store` to one location, so 1.00 means the store-to-load hop is free and the add is the whole cycle:

| Location | 64-bit | 32-bit | 16-bit `movzx` | 8-bit `movzx` |
|---|---:|---:|---:|---:|
| `[reg + disp]` | 1.00 | 1.00 | 8.00 | 8.00 |
| `[rsp + disp]`, `[rip + symbol]` | 1.00 | | 8.00 | |
| `[reg + index*8 + disp]`, `fs:[reg + disp]` | 1.00 | | | |
| `fs:[disp]`, what compilers emit for thread-locals | 6.5 to 7.1 | 6.7 to 7.2 | 8.00 | 8.00 |
| `ds:[absolute]` | 5.9 to 7.1 | | | |

The free hop needs a 32 or 64-bit access, or a read-modify-write instruction of any width (`add [reg
+ disp], 1` is 1.00 at 64, 32 or 16 bits), and an address with a base register. The register may
change: copied, recomputed or reloaded every iteration it is 1.00 to 1.13, and a load and a store
through different registers holding the same address are 1.00. The position in the code does not
matter either: the inlined free ran 2.37 to 2.48 cycles at all 88 placements, with the load and the
commit at every position in a line and across line and page boundaries. By inference this is the
core's memory renaming, which forwards the stored register to the load at rename time.

**In compiled Rust.** A `thread_local!` with a `const` initializer compiles to `fs:[constant]`. Two
floors that differ only in where the free-list heads live cost 7.3 to 7.4 a `Box` pair with the
heads in a `thread_local!` array and 4.0 behind a thread-local pointer. Compilers choose this form
on their own, which is one reason the fast path is assembly. The price of assembly: a compiler that
sees an allocation and its free together folds a plain-Rust list away, to 1.3 to 2.1 cycles, and an
`asm!` path is opaque to that.

**`call [mem]` ends renaming across the call.** A pair of empty calls costs 10.00 through `call
[GOT]`, the same as through the PLT, against 8.00 direct. With a counter in memory in each callee it
costs 16.01; with Rust's rseq pop and push 16.11; with the C `malloc` and `free` 26 to 33. Direct
and PLT calls stay at 8.00 and 10.00 whatever the callee.

Every hop that crosses a `call [mem]` costs 7 cycles; a hop inside one call stays free. `call [mem]`
retires 5 ops where `call sym` retires 1, so by inference it is microcoded, and microcode ends
renaming. The C routines suffer most because their state is in memory twice (slab word and slot),
and free's word address hangs off the pointer the previous pop loaded. `call reg` measured 9.00
here, but read 8, 10 or 12 a pair to the generated routines, changing between repetitions.

Who calls this way: gcc's `-fno-plt`, and rustc for calls into other shared objects. So a Rust
program on the `System` allocator with cralloc preloaded pays 28 cycles a pair. A shared tcmalloc
built with gcc's default TLS descriptors pays it twice per `malloc`, through `call [rax]`: 41 to 57
cycles a pair against 14 to 15 with initial-exec TLS. In bursts the hops are not on a loop-carried
path, and cralloc is the fastest there at 16.2 to 16.8.

**Rule.** State the fast path both writes and reads is 64 bits wide and addressed through a base
register. **Enforced by Lean** for our routines: a memory operand in the instruction model always
has a base register, so `fs:[constant]` cannot be written, the model has byte loads, used for the
tables, and no 16-bit load from memory, and a routine's contract is a 64-bit word. A caller's `call
[mem]` is outside any rule of ours. (Evidence: forwarding 2026-10-16, placements 2026-10-18 at
`e52974b`, calls 2026-10-17; 2 to 40 million iterations, spreads under 0.05 cycles in pair cells.)

### A store whose address arrives late holds up the next load

**Mechanism.** In a loop of mixed sizes, the next `malloc`'s slab-word load is held behind the
previous `free`'s commit store. A C `free` has only a pointer, so that store's address comes at the
end of a chain: word load 4, slot load 4, shift and subtract 2, region table 4, `lea` 1. A counter
shows the hold (`ls_bad_status2.stli_other`, 0.5 to 1 a pair), with no mispredicts or resyncs. One
size class runs at the floor of 8.00; two classes alternating already cost 17.8 to 20.7. By
inference, the memory-dependence predictor learns the true same-class dependence and then makes
every pop wait for the youngest push.

| Free | Pair, direct / PLT | Mixed, direct / PLT | `call [GOT]`, pair / mixed |
|---|---:|---:|---:|
| Class from the address (C's `free`) | 8.00 / 10.00 | 21.9 / 23.6 | 25.8 / 26.1 |
| Class by arithmetic from the region index (a pretend heap) | 8.00 / 10.00 | 19.0 / 19.7 | 24.5 / 22.5 |
| Class from the size, the address checks it (C's `free_sized`) | 8.06 / 10.00 | 11.2 / 11.5 | 16.1 / 13.7 |
| Class from the size, trusted (Rust's `dealloc`) | 8.00 / 10.00 | 9.6 / 10.1 | 16.2 / 13.4 |

With the size trusted, pairs and bursts cost nothing more; the call is the cost. In the mixed loop
the missing size costs about 12 cycles a pair. The region table's load accounts for only 2.9 to 3.9
of that and the rest is the pointer chain, so changing how regions are assigned would not close the
gap; a size from the caller does. Rust gets this by construction: the layout comes with every free.

**C can get it safely.** `free_sized` finds the class from the size, so the store's address no
longer waits on the pointer. It reads the class from the address in parallel and compares, a
predicted branch off the chain. A mismatch goes to the slow path, which uses only the address, so a
wrong size behaves exactly like a free with no size. The check costs nothing through the PLT; with
direct calls it costs 0.06 a pair, 0.35 an element in bursts of 8 and 1.4 in bursts of 32, by
inference the price of sitting at 24 micro-ops.

**A predicted branch's operand still costs.** A routine that loads the mask itself saves the
run-time case a load, but costs constant-size calls more: a mask at a fixed distance from the word
saved 0.66 cycles on the run-time pair and cost 0.38 on the `Box` pair; a mask in the slab saved
0.64 and cost 0.84. A probe that changes nothing but where the constant path's mask comes from, a
load instead of an immediate, costs 0.36 on the `Box` pair. The extra cycles are where nothing
retires while a load is outstanding, with no more blocked loads. Why was not found. **Rule:** the
inlined routine takes its mask in a register, and a constant class folds it to an immediate. The C
routines read it from memory, since they have no constant case. (Evidence: 2026-10-17 for C, best of
3 over 2 or 3 rounds, spread 0.05 in pairs, up to 0.8 in mixed cells; 2026-10-22 at `198fc46` for
Rust, four placements, three rounds.)

### The front end sets a cliff by micro-op count and by lines, not by alignment

A straight-line run goes from a branch target to the next taken branch. Cycles per iteration of a
loop of N fused micro-ops with no dependency between iterations (one-byte `nop`s and two-byte moves
give identical results):

| N | 2 to 8 | 9 | 10 | 11 | 12 | 13 to 24 | 25 to 30 |
|---|---:|---:|---:|---:|---:|---:|---:|
| Cycles | 1.00 | 1.13 | 1.25 | 1.38 | 1.50 | 3.00 | 5.00 |

With a 2-cycle dependency chain in the loop the figures are 2.00 up to 12, 3.00 for 13 to 24, and
5.00 from 25. Breaking a fusion with a `nop` between a `test` and its `jz` costs a whole cycle at
the floor: 3.00 against 2.00. By inference, the op cache hands over two entries of up to six fused
micro-ops a cycle.

**A call hides most of it.** A call and return take 4 cycles of the front end whatever the function
does, and overlap unrelated work. A function whose body stays in one line costs 4.00 a call up to 24
micro-ops and 6.00 from 25 to 30.

**Lines**, from routines of 88 and 89 bytes swept over all 64 entry offsets in two builds:

- Crossing one line boundary is free wherever it falls. At up to 13 of about 40 fast offsets a batch
  costs about a cycle more, moving with addresses rather than the offset.
- Reaching a third line costs a cycle a call, even if only the `ret` is there. At 101 bytes
  `c_malloc` costs 4.08 a call at entries 0 to 25 and 5.08 past them.
- A short tail costs a cycle a call: routines of 67 to 71 bytes at offset 0, with only their last
  one to three instructions in the second line, cost 9.00 a pair against 8.00. A 75-byte routine was
  slow in one batch sweep and no other, which is why the limit runs to 75.

**A short tail is op-cache misses, and it applies to any straight run,** including the caller's code
around an inlined fast path. Slow cells of small allocator-free functions had exactly one miss per
call (one exception), with more than 12 of the run's fused ops before a line boundary and the run
ending one to four instructions after it. That is not sufficient: one function met it at four
entries and was fast. By inference, the op cache keeps no entry for such a tail, so the decoders
deliver it on every pass.

**It costs more when the next load is waiting.** In the tree job, which builds, walks and drops a
binary tree of `Box`es, `drop_in_place`, byte for byte, costs 9.02 a free at most entries and 10.03
at entries 30 to 37. Dispatched and retired ops are equal and the forward count is unchanged, so
nothing is flushed and the hop is not waiting; what changes is one or two op-cache misses a free.
Over memory each miss costs 7 to 9 cycles because the bubble delays the next node's load, and a
cycles profile blames that load. In a build where that function's entry moved from offset 0 to 32,
the job's drop phase ran 8.7% slower (medians of five). Rust aligns functions to 16 bytes, so such a
function is slow at one of the four places a link can put it.

**Nothing in the routine decides this.** The run that misses is the caller's, and the same cost
appears with no allocator code at all. So the spread to expect from an inlined fast path is the
caller's: 0 or 1 cycle a call with its data in L1, 0 or 7 to 9 when it waits on memory. Out of line
we set the placement ourselves. A cycle of placement on a short hot routine is common on this core:
mimalloc's free moves by 0.9 cycles (20%) with its own address, with no op-cache misses, so by
another front-end effect, not diagnosed; tcmalloc was not measured. The program's own code moves
too: half of serde_json's `Value::deserialize` is one 16-byte reload from the stack, and the JSON
job's parse phase moved by 7% with code layout.

**Rejected:** a taken `jmp` to split the run. It removes the misses and costs more than it saves:
the drop slows at entries 28 to 47 instead of 30 to 37, a hand-kernel inline pair costs 4.89 to 5.05
against 4.51, and `c_free` costs 5.07 at every offset. Aligning the inlined block was not measured:
it would execute up to 63 bytes of padding on every free and only move the boundary into the code
after it.

**Rule.** Out of line, a routine starts on a line, its hot path has at most 24 fused micro-ops, and
it is at most 64 bytes or from 76 to 128. Inlined, it has at most 13, one more than the 12 the core
delivers at full speed; that keeps the routine small and does not bound the run it lands in, since
an inlined pair is one straight run of 33.7 ops and the caller's code extends it. A compare stays
adjacent to its branch. Slow paths are out of line. **Enforced:** `Rust.checkUops` counts micro-ops
in Lean when it emits and refuses code it cannot count, and `lean/scripts/check_sizes.py` assembles
every naked function and checks bytes and alignment. Both run in `lean/scripts/check.sh`. Today
(that script, and micro-ops counted from the generated text, 2026-11-06) an inlined pop or push is
12 micro-ops in the default form and 11 in the executable form, plus the caller's test on a pop;
`c_malloc` is 103 bytes and 20 micro-ops, `c_calloc` 110 and 22, `c_free` 120 and 23, and
`c_free_sized` 128 and 24, at the limit of both rules. (Evidence: 2026-10-16 to 18; best of 3 over 2
to 5 interleaved rounds, rounds within 0.05, counters unmultiplexed.)

### Reaching the path: inlining, LTO and how the call is made

`__rust_alloc` and its siblings are generated in the crate that declares `#[global_allocator]`.
Whether a call site inlines the fast path depends on whether the optimizer can see that crate from
there, for a plain-Rust fast path and an `asm!` one alike.

| Profile | Binary crate | Dependency crate | Inside std (`format!`) | Marker call per allocation |
|---|---|---|---|---|
| `lto = "off"` | Inlined only in the shim's own unit | Call | Call | Yes |
| Default `release`, or `codegen-units = 1` | Inlined | Call | Call | Yes |
| `lto = "thin"`, any number of units | Inlined | Inlined | Call | No |
| `lto = "fat"` | Inlined | Inlined | Partly | No |

The marker (`__rust_no_alloc_shim_is_unstable_v2`) is an empty function std calls before every
allocation, through the GOT without cross-crate LTO. Alone it costs 5 to 6.5 cycles a pair, more
than the allocation. A stand-in `asm!` magazine costs 5.8 a `Box` pair inlined with no marker, 12.3
inlined with it, and 15.3 out of line. A library cannot set any of this for its users.

**A thin-LTO hazard for assembly fast paths.** LLVM's thin LTO will not import a function containing
inline assembly out of a module that also has a private symbol in `llvm.used` or in module-level
assembly. So a crate that declares the allocator and has a private `#[used]` static or a
`global_asm!` block with a local symbol silently stops inlining an `asm!` fast path. Declaring the
allocator in a crate of its own makes thin LTO reliable, and gives up inlining in builds without
LTO. **Rule:** recommend `lto = "fat"` and offer the small declaring crate.
`bench/roofline/fastpath/check.py` fails if the hot loop of each kind of call site stops holding
both sequences and no call; it is run by hand, and nothing runs it automatically.

**What `asm!` allows.** A `label` operand cannot be combined with a real output on stable
(rust-lang/rust issue 119364). So a push branches straight to its slow path, and a pop reports a
miss through its result, at the cost of a fused test the caller makes anyway.

**One load of `__rseq_offset` or two.** In an executable, `mov reg, [rip + __rseq_offset]` becomes a
copy relocation, correct from the first user code. In a shared library it does not link, so a wrong
choice is a link failure, never a run-time fault. Inlined, it is worth 0.3 to 1.0 cycles a pair (the
two forms' difference over eight loops at `198fc46`); out of line, nothing. It is the `executable`
cargo feature; the default, the C entry points and the preload library use two loads.

**Code size.** Inlined under fat LTO an allocation is about 85 bytes in place and 165 out of the
way, a free 70 and 145, counting each site's 32-byte descriptor, 72 bytes of relocations and 16-byte
word; a run-time size adds 35 to 40 in place. An out-of-line call is 5 to 10.

## What bounds the inlined pair today

**The hot loops run close to the core's width.** Instructions divided by cycles from the runs behind
the Rust table (executable form, 2026-10-22 at `198fc46`, mean over four placements):

| Loop | Cycles | Instructions | Instructions a cycle |
|---|---:|---:|---:|
| Pair or batch, constant | 4.96, 5.36 | 40, 43.3 | 8.1 |
| Pair or batch, run-time size | 7.26, 7.29 | 56, 57.3 | 7.7, 7.9 |
| Pair or batch, mixed sizes | 8.93, 9.71 | 53, 60.5 | 5.9, 6.2 |
| Churn | 23.9 | 90.5 | 3.8 |

A fused compare and branch is two instructions and one op, so instructions a cycle can pass 8.
Counters from 2026-10-17, on an earlier build with the same 40 instructions and 4.96 cycles, put the
constant pair at 33.7 ops. At 8 a cycle retirement alone would take 4.2 cycles, and retirement waits
on a load for 0.51; that leaves 0.24 unaccounted. The front end is the other candidate: its table
gives 5.00 for a straight run of 25 to 30 ops, and this pair is one straight run. Nothing here
separates them.

**Past that, the chain through the CPU number.** Each call needs two dependent loads and a shift
before its slab-word load: about 9 to 10 cycles in the executable form, 13 to 14 in the default
form. In a loop, the next call's loads overlap the last call; a lone call waits that long whenever
the caller needs the pointer soon.

Taking inputs as `in(reg)` removes 5 register copies: 28.8 ops, 4.74 cycles, 0.87 waiting. Forming
the word's address in one register adds a `lea` to the chain: 31.0 ops, 5.26 cycles, 1.20 waiting.
So removing ops makes the wait grow by about what it saves, and lengthening the chain costs one for
one. This chain is what per-CPU addressing needs, so compiled Rust's inlined pair stops near 4.7 to
5.0 in the executable form and 5.4 to 5.5 in the default one. Over eight interleaved rounds
`in(reg)` saved 0.04 to 0.33 a pair and cost 0.29 on one batch loop, which is not explained. The
routines keep fixed registers, which is where their contract reads the inputs.

**The lookup costs instructions, not latency.** With a run-time size the call site adds 8
instructions outside a loop and 6 inside: a compare, a shift, the class-byte load, a `lea` for the
word and a load for the mask. From the size to the word's address takes about 9 cycles, level with
the CPU number in an executable and hidden behind it in the default form. Nothing waits on the mask
but a predicted branch: built with an immediate (correct only for the size it ran), the loop ran no
faster.

- The word is a pointer offset, not an integer sum, or LLVM turns the sum on a page-aligned static
  into a second instruction, `or`.
- Class bytes, masks and slot sizes share one page-aligned static, so one base address serves every
  load.
- A load is 4 cycles through `[base]`, `[base + disp]` or `[base + index]`, and 5 with a scaled
  index or with an index and a displacement. C's region load uses the 4-cycle form; Rust's class
  byte is the 5-cycle form, and its latency is hidden.

The change that gave this shape saved 0.21 to 0.55 cycles on the run-time loops in the executable
form, moved the default form by -0.32 to +0.08 and did nothing on constant sizes. On the log store
it read 1.6% over five placements and two rounds; the instructions saved predict a few tenths of a
percent, so read that as no regression. 19 of 25 small-object kernel jobs got faster, by up to 8.5%;
the three that read slower are level over placements or sat in a job's slow mode.

**The latency of this chain can reach far past its cycles.** In an eight-stage pipeline each stage
frees an object whose size it reads from the object, which another core wrote, so by inference the
class chain and not the CPU number is the long one. The same change took the job from 4.18 to 3.48
ns an allocation, and adding three cycles of latency to the word's address gave it all back, 4.26.
Slow paths, depot traffic and retries were level in all three builds. So were store-to-load
interlocks (0.53 to 0.59 an allocation), and cycles waiting on a load fell by only 6 of the 36
saved, so this is not the hold of the section on late stores. A two-sided pipeline runs at the pace
of its slower side while the other spins, so a few cycles a call may change which side that is; the
mechanism is not known. (Three interleaved sessions, runs within 0.06 ns.)

**Not built:** one table load giving both the word's offset and the mask (it shortens only the
mask's chain, at eight times the footprint); computing the class for small sizes (a branch on the
size, which mispredicts where sizes vary); a smaller class table (the byte table already touches one
line for every size up to 512).

## Why a pair costs more in real programs

On four of the seven profiled server jobs a pair costs 12.3 to 14.9 cycles inlined and 17.7 out of
line, against 5 to 7.3 in the loops; jemalloc's costs 15.2 to 19.3 and 27.4 on the same jobs
(2026-10-22 at `8ad5150`, before the shorter lookup; the sums of allocation and free, whose split
moves with sampling skid). `servers.md` has all seven: about 14, 11 to 18. The fast path is 10.6% of
the log store's cycles, 5.3% of Meilisearch's, 2.9% of the order service's and 1.1 to 1.3% of the
RPC jobs'. From what was measured, not a new experiment, the difference is:

- **A shared core.** The server figures have an idle-priority spinner on each core's SMT sibling,
  where the loops have the sibling idle. That spinner raises jemalloc's cycles a request on the
  order service by 17% (`measuring.md`), so part of this gap is the spinner. How much was not
  separated.
- **Misses.** The allocator's own L1 misses are about 180 a request on the store against about 5,200
  calls: one call in thirty misses on a slab word, a magazine slot or a table, mostly to L2, about
  15 cycles. That alone adds about half a cycle a call. Op samples inside the pop fall on the slab
  word's load (26%) and the slot's (37%).
- **Mixed sizes.** Each class has its own slab word and magazine, so many classes touch more lines:
  4.5 a call against 3.6, all in L1.
- **The CPU number's latency.** A lone call waits for it, where a loop overlaps it.
- **The front end.** The inlined copies are spread over the program's code, and a copy that misses
  the op cache or ends a line with a short tail pays a cycle or more.

## `realloc` should do the least the case needs

`realloc` of small blocks was the largest allocator cost left on the servers, 2.5% of the RPC jobs'
cycles, and every call moved a `Vec`, `String` or buffer one class up. The old path was the general
resize inlined whole, with the class computed five times.

**Rule.** Every resize tries a lean path first (`heap/resize.rs`): each class found once with the
inlined lookup; same class, the block stays; otherwise pop, copy, push. Anything else goes to the
general resize, `#[cold]` and out of line, which decides exactly as before. A `Vec<u8>` growing from
128 to 256 bytes went from 354 instructions and 63 cycles a `realloc` to 126 and 23, counting the
call, the copy and the collection's growth code. C's `realloc` went from 375 and 66 to 273 and 54.
`__rust_realloc` went from 12 KiB of code to under 400 bytes.

On the RPC jobs that is about 4% fewer instructions a request (12,200, about 222 a call), and 1.4%
on the order service. Over five code placements against main `18ca2d9`, the HTTP/2 RPC jobs ran 2.1%
(gRPC, three runs per placement) and 1.9% (Connect, two) faster, ahead at every placement; the order
service (-0.7%) and another RPC job (-0.7%) are within their noise. That the other jobs did not move
is not established: `vecgrow 1x256Mi` runs in two modes, and one round read +1.9% with its
instructions level. Calls the lean path does not take cost slightly more: small to large 1,866 to
1,924 instructions, large to large 5,751 to 5,786.

C cannot use the shortcut `free` uses. `free` reads a mirror of the region table and does not check
that the pointer starts a slot. `realloc` copies from the pointer, so an interior pointer would copy
from the neighbouring slot. It reads the back end's own table and checks the slot
(`Back::small_slot`), about 70 of its instructions. (Evidence: 2026-10-22, a growing collection
against a presized control, medians of three interleaved runs; per-call figures against main
`35e31c7`.)

## The C entry points

C has to find the class at run time, so its routines add a front half to the same pop and push. What
was chosen, on the 8.00 floor of two direct calls:

- **A byte table at `(n + 7) >> 3` covers every small size,** with C's 16-byte rule built in: 8.00
  direct and 10.00 through the PLT at every size from 0 to 32,768. A two-range lookup costs 8.55
  direct; a class computed with `bsr` 9.69.
- **Every byte that is not a small class has a zero mask,** so the push's own mask test sends large
  classes, run heads and tails and unassigned regions to the slow path. A separate test would be 2
  micro-ops and 12 bytes, past the limits.
- **One unsigned compare of the region index against 4096** rejects pointers below and above the
  heap and, with the heap's words still zero, every real pointer. Null returns at once in `free`;
  the sized free leaves it to the compare.
- **Every miss is a tail call with the arguments intact** (`malloc_slow(size)`, `free_slow(ptr)`),
  through a stub after the `ret`. The front half stays in the routine: a jump to a shared one costs
  2 cycles a pair through the PLT. Every routine arms first, so an abort restarts the whole of it.
- **What costs a micro-op and no cycle:** `shr r, cl` for `shrx`, so BMI2 is not needed.
- **The PLT costs exactly one cycle a call,** one more taken branch to fetch; `endbr64` costs
  nothing more. A burst loop through the PLT can move by a cycle an element between identical builds
  (10.07 against 11.23): placement of the caller, the stubs and the routines.

**`calloc` pays for a routine; `realloc` does not.** A generated `calloc`, for sizes up to 1 KiB
through a byte table indexed by the size, costs 11.05 at 16 bytes and 13.54 at 256, against glibc's
14.04 and 17.13 and mimalloc's 14.14 and 20.62. Its zeroing tail uses its own 16-byte stores up to
32 bytes and glibc's `memset` above: one `memset` for every size cost a cycle at 16 and 32 bytes (a
call through the GOT and glibc's dispatch), and stores up to 512 bytes were 1 to 8 cycles slower
from 64 bytes up. `realloc` in place, in a C stand-in with size and region lookups and a compare,
cost 16 to 18 cycles against glibc's 17, mimalloc's 25 and jemalloc's 60; the shipped path adds the
slot check and was not re-measured. The aligned sized free gets no routine, since its prediction
would cost 3 micro-ops on a routine already at 24: it is plain `free`. (Evidence: 2026-10-17;
candidates in hand-written assembly, then the generated routines in the real library, best of 3 over
2 or 3 rounds; incumbents in their production builds.)

### C++ new and delete without our operators

The preload library exports no C++ operators (`docs/design.md`, "Interfaces"), so a C++ program's
`new` and `delete` run libstdc++'s, which call our `malloc` and `free` through libstdc++'s PLT. A
sized pair of 32 bytes costs 17.0 cycles through the PLT against per-CPU tcmalloc's 13.0, mimalloc's
15.2, glibc's 20.0 and jemalloc's 20.1, and 29.3 under `call [GOT]` against 17.4, 16.2, 23.3 and
38.8 (2026-10-17, CPU 12, best of 3 interleaved rounds). Exporting the deletes bought 1 to 3 cycles
through the PLT and 13 in the `call [GOT]` pair, and a throwing `new` still went through libstdc++'s
wrapper, leaving 15.0 against tcmalloc's 13.0. A full export is estimated, not measured, at 10 to 11
through the PLT. That layer was built and removed: the last commit with it is `d12eacf`.

## What turned out not to matter

Each of these was measured, and none bought a cycle at a fast placement.

- **Byte alignment by itself.** The magazine pair runs 2.00 with its loop start at every byte offset
  from 0 to 63. A function of 6 or 11 micro-ops costs 4.00 at any offset, straddling a line or not.
- **Where the store and reload sit in the code.** The hop is free at every placement measured.
- **Bytes.** Of twelve encodings of the out-of-line routines, from 89 bytes down to 67, none gains a
  cycle: 8.00 a pair at a fast placement. Several combined land in the 65 to 75-byte range and lose
  one.
- **A micro-op, out of line.** Taking the mask from the slab's base register removes a `lea` and
  does not show. A table that gives the register value directly saves one and buys nothing.
- **The sign test for the CPU number,** `test; js` for `cmp; jae`: 4 bytes shorter, the same
  micro-ops. It buys nothing inline (5.24 against 5.27; 4.32 against 4.23), and with it a CPU number
  of 8192 or more would index beyond `SLABS`, so memory safety would rest on the kernel's numbers.
- **Per-class out-of-line entries.** They beat the generic out-of-line form by 0.5 a `Box` pair and
  lose to inlining by 1.2 to 2.
- **An inlined `call` to the slow path,** to drop the pop's test. `Box` and `Vec` repeat the test
  anyway. With no branch left in the pop, LLVM unrolled the 32-allocation loop whole, which ran 6.85
  against 5.96, and 13.6 against 5.5 with the direct load. The block also needs a stub that
  preserves vector state and realigns the stack.

One number looked like a finding and was not: a default-release `Box` pair at 29.6 cycles did not
reproduce (13.7 over 30 runs). The same binary read 16.7 to 22.0 under heavy load on other cores, so
it was most likely interference.

## What is left

Ranked by what it might buy; none is measured except where a number is given.

1. **The CPU-number chain,** inside the routines. Reading the rseq area's address once per function
   would share it between calls. It needs new Lean routines and is the largest latency left on a
   lone call.
2. **Misses on the allocator's own lines.** Hot words of different classes are on different lines
   (one 32-byte chunk per class, two classes to a line); two to a chunk would cost a run-time-size
   pair two cycles of arithmetic.
3. **The moving `realloc`:** about 20 of its 85 instructions beside `memcpy` move values around the
   routines' fixed operands.
4. **C's mixed-size loop:** 22.6 to 23.7 against glibc's 14.2. glibc's pop gets the pointer with one
   dependent load, where a magazine needs two, about 4 cycles of the chain. The C free reads the
   heap's shift and base as two published words; one packed word looked about a cycle better.
5. **C under `call [GOT]`:** 28.2 against mimalloc's 16.1. About 1.3 cycles is the order in the C
   routines' pop: `lea rcx, [rcx - 8]; mov rax, [rcx]` makes the popped pointer wait for the `lea`.
   The inlined pop already loads first, and `c_malloc` and `c_calloc` have the byte it costs to
   spare. This is inference from one interleaved comparison.

## Open questions

- **A second CPU model.** Everything is one Zen 5 part. `hardware.md` says which rules should carry.
- **What limits the constant pair, and what its last 0.24 cycles are.** 33.7 ops at 8 a cycle is
  4.21 and the wait on loads 0.51, of 4.96. The front end's table gives 5.00 for a straight run of
  25 to 30 ops, but 28.8 ops at 4.74 shows that is no hard bound.
- **Why a predicted branch's operand delays a loop** (a mask from memory costs 0.4 to 1.0 cycles a
  pair on constant sizes).
- **Whether the mask rule is right for servers.** It was decided on loops. Weighted by the RPC
  server's sites (1,134 of 1,820 run-time; sites, not calls), the loop differences net about -0.27
  cycles a pair for the fixed-distance form: under what a server run can resolve, and it needs new
  Lean routines.
- **Why the eight-stage pipeline responds so far out of proportion,** and which side sets its pace.
- **Why the tree job's drop misses about one node in ten** where the kernel misses on every node.
- **Up to a cycle of the mixed-loop differences between C shapes is unexplained,** and identical
  builds move by up to 0.8 there.
- **How much survives in a server.** Real programs put other work between an allocation and its
  free, so the chains and windows here are worst cases.
