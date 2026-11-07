# Roofline harness

Small, dependency-free programs that measure what an allocator fast path can cost on the
host, and what the kernel and the Rust toolchain add around it. The results and what they
mean are written up in `docs/research/roofline.md`, and for aarch64 in
`docs/research/firestorm.md`.

Each directory is a stand-alone cargo project (none belongs to a workspace). `aarch64/` and
`machine/` are any aarch64 Linux, `oscost`'s `faults` runs on both, and everything else is x86-64
Linux only. Everything needs `perf_event_open` for user-space counters
(`/proc/sys/kernel/perf_event_paranoid` of 2 or lower).

Run measurements on a quiet machine. Programs pin themselves; builds should be kept off the
measurement CPUs (`taskset -c 4-7 cargo build ...`). Defaults assume the reference machine,
where CPUs 2 and 3 are two cores of one L3, 18 is the SMT sibling of 2, and 10 is in the other
L3; override with the environment variables below.

| Directory | Measures | Run |
|---|---|---|
| `kernels/` | Hand-written assembly fast paths, store-to-load forwarding, atomics, call cost, core widths, and a code-layout sweep. Cycles, instructions, retired uops and forwards per operation. | `cargo run --release -- [prefix,...]` |
| `rust-alloc/` | Allocation workloads through Rust's global allocator for glibc, mimalloc, jemalloc and two floor allocators, single-threaded and cross-thread. | `./run.sh [workload-prefix]` |
| `fastpath/` | A check that fails if cralloc's own fast path stops inlining under fat LTO: the hot loop of each kind of call site must hold both rseq sequences and no call. Prints each loop's bytes. Seconds. | `taskset -c 4-7 ./check.py` |
| `oscost/` | Cost of mmap, page faults, the madvise family, mprotect, guard pages and zeroing; and (`faults`) demand faults against populate stretches and huge pages, one to four threads, huge-page advice around a slot, partial purges of huge pages, khugepaged on sparse memory. | `cargo run --release`, `target/release/faults single\|threads\|advice\|split\|khuge` |
| `rules/` | What the heap's rules do on the kernel it runs on, through the real allocator: where the retained cap and the floor act, whether a class gets huge pages, who splits them, what a stagger does for buffers written in turn, and what a first touch under huge-page advice costs when free blocks of that size run short. | `target/release/roofline-rules cap\|floor\|huge\|split\|stagger\|fault` |
| `machine/` | What an aarch64 machine charges an allocator, on a kernel of any page size: the coherence unit, moving a line, caches' sets and ways, TLB reach and sets, prefetchers, stagger, class origins, the kernel's calls, huge pages' worth, the clock and idle. | `./run.sh OUT`; see below |
| `centry/` | The C entry points: candidate `malloc`, `free` and `calloc` routines reached directly, through the PLT and through the GOT; a preloadable stand-in around the best of them, compared with glibc, jemalloc, mimalloc and tcmalloc from plain C loops. | `./build.sh`, then `./run.sh run PATTERN` and `./compare.sh` |
| `chain/` | The slab word's store-to-load chain against code placement: the generated inline free and allocation, the tree job's drop, and `c_free`/`c_malloc`, each placed at every offset in a 64-byte line and across 4 KiB, read from the generated files; and mimalloc's free and malloc moved the same way. | `cargo run --release -- FORMS [PLACES]`; see below |
| `toolchain/` | The relocation a direct load of glibc's `__rseq_offset` gets, in an executable and a shared library, and the code rustc generates around an `asm!` block that calls. | `./check.sh` |
| `aarch64/` | The same questions on any aarch64 Linux: widths, latencies, forwarding and renaming, atomics, the front end, page-offset holds, the floors, candidates for the routines and the C entry shapes. `reach/` builds a fast path every way a conditional branch or a relocation could fail; `stale/` measures how long a scratch register survives in real programs. | `./run.sh [pattern,...]`; see below |

## aarch64

Built for an Apple M1 Max under Asahi Linux, and portable: the same commands run on a Graviton or a
Grace. The core's PMU is the one whose `cpus` file in `/sys/bus/event_source/devices` names the CPU
(`apple_firestorm_pmu`, `armv8_pmuv3_0`), cycles and instructions come from its `events` directory,
and raw events are per PMU and optional. Kernels use the base instruction set, with LSE and RCpc in
the atomics family only.

```
cd aarch64
../../../scripts/heavy cargo build --release
./run.sh w_,l_,z_                   # widths, latencies, which pairs fuse
./run.sh f_,r_,h_,rk_               # forwarding, renaming and its rules, hops that cannot rename
./run.sh a_                         # atomics and barriers, alone and beside other work
./run.sh fe_                        # front end: length, position, taken branches, calls
./run.sh p_,c_,b_                   # the floors and candidates: pair, two calls, batch of 32
./run.sh cl_,cm_,cs_,cc_,sl_        # C entry shapes and the slow path's routines
./run.sh m_,mb_,pw_,pwn_,ps_,pc_    # mixed classes, a caller's work, placement sweeps
./run.sh offsets                    # does a load wait on a store with its low address bits?
./run.sh clock                      # is a cycle a fixed time, from cold and warm?
./run.sh list f_                    # the kernels and what each runs
```

`run.sh` runs through `scripts/measure`. A pattern ending in `$` must match a whole name. The
program pins itself: `RF_CPU` (by default the last CPU but one), `RF_ITERS` (4,000,000), `RF_REPS`
(4, the first a warm-up), `RF_ROUNDS` (3, every kernel once a round, interleaved). A cell is the
fastest repetition over all rounds, and `+x` how far the slowest round's fastest sits above it.

`RF_EVENTS` names up to a few more counters: `name=0xCODE`, a name in the PMU's sysfs directory, or
for Firestorm a name in the table in `src/main.rs` (`docs/research/firestorm.md` has the ones
validated). Firestorm's events are tied to subsets of its counters, so more than two or three can
multiplex even with counters free; a multiplexed repetition is dropped and the program says so.

The kernels' sources are Python modules in `kernels/`, one per family, run at build time; each
module's docstring names its options. The program links without PIE, since `adrp` to
`__rseq_offset` needs a copy relocation that GNU ld refuses in a PIE (`reach/`).

```
cd aarch64/reach && ../../../../scripts/heavy ./check.sh      # about a minute
cd aarch64/stale && cc -O2 -fPIC -shared -ffixed-x18 probe.c -o probe.so -ldl
LD_PRELOAD=$PWD/probe.so PROGRAM ...                          # the summary goes to stderr
```

## kernels

```
cd kernels
taskset -c 4-7 cargo build --release
./target/release/roofline-kernels                 # everything, about a minute
./target/release/roofline-kernels p_,c_,b_        # fast-path shapes: pair, call pair, batch
./target/release/roofline-kernels k_,m            # forwarding, atomics, calls, widths
./target/release/roofline-kernels l               # layout sweep
```

Kernel name prefixes: `p_` alloc+free pair inline, `c_` the same through two calls, `b_` 32
allocations then 32 frees, `k_` and `m` single-mechanism probes, `l` layout sweep (generated
at build time by `gen_layout.py`, which needs `python3`). The kernel sources in
`src/kernels.rs` are the reference for exactly which instructions each name runs.

`RF_CPU` (default 2) is the CPU to pin to; its SMT sibling should be idle. `RF_ITERS`
(default 40000000) is the iteration count. Each kernel runs four times; the first is a warm-up
and the fastest of the rest is printed. The rseq kernels count aborts and say so on stderr.
Raw events 0xC1 (retired uops) and 0x35 (store-to-load forwards) are AMD-specific; on other
CPUs those two columns are meaningless.

### The proved slab routines

`gen_routines.py` (run at build time) emits hand-written copies of the routines in
`crates/cralloc/src/front/generated/`, instruction for instruction, and variants that each
change one thing: their placement in a cache line, their encoding (short jumps, registers,
address arithmetic, where the mask table sits), the CPU-number test, how `__rseq_offset` is
read, per-class entries, and an inline pop that calls its slow path. The script's docstring
names every variant.

```
./target/release/roofline-kernels routines sizes                 # bytes and fused uops of each
RF_ITERS=10000000 ./target/release/roofline-kernels routines sweep gen,sj   # entry offset 0..63
./target/release/roofline-kernels routines place gen@0/0,gen@e/e  # chosen pop/push offsets
./target/release/roofline-kernels routines inline                # inline forms
python3 analyze_sweep.py SWEEP_OUTPUT                            # slow offsets vs. layout
```

A sweep moves one routine through every offset with the other at offset 0, in a pair loop and a
batch loop of direct calls. `e` places a routine so that its hot path ends on a line boundary.
Items are run in `RF_ROUNDS` interleaved rounds (default 3); each cell is the fastest
repetition, and `+x` how much slower the slowest round's best was. The run ends by printing how
busy `RF_WATCH` (default 3,18,19) was.

`rust-alloc/run_slab.sh` builds a stand-in allocator around the same sequences (`src/slab.rs`,
cargo features `slab`, `slab_direct`, `slab_call`, `slab_nonnull`, `slab_ool`, `slab_pc`) under
fat LTO and runs the single-thread workloads; `RF_CPU=3` keeps it off a CPU a sweep is using.

## chain

```
cd chain
taskset -c 12-15 nice cargo build --release
RF_EVENTS=ocmiss,ldwait,stli ./target/release/roofline-chain td,if,cf 0-63   # under flock and memlimit
./build-incumbent.sh OUT && ./run-incumbent.sh OUT 2                      # mimalloc, offsets 0 to 63
```

`gen.py`'s docstring names the forms. The kernels take the routines' text from
`crates/cralloc/src/front/generated/` at build time, so they always run what the generator emits.
`RF_EVENTS` takes up to three events by name (the table in `src/main.rs`) or `name=0xCONFIG`, on
plain counters; a multiplexed repetition is dropped. `RF_CPU` (2), `RF_OPS`, `RF_REPS`,
`RF_ROUNDS` as for the kernels.

## centry

```
cd centry
./build.sh                                   # gen.py, the library, the drivers, the stand-ins
./run.sh sizes                               # bytes and fused uops of every routine
./run.sh run 'kp_mLj_fUjn_*,kb32_*_[dp]'     # kernels by name (fnmatch); see gen.py's docstring
./run.sh --ibt run 'kp_*_p'                  # the same driver linked with the IBT PLT
./compare.sh pair:32 burst:32:32 mixed:1:512 # C loops (cloop.c) under each preloaded allocator
```

`RF_CPU` (default 13; its sibling 29 should be idle), `RF_ITERS`, `RF_REPS`, `RF_ROUNDS` and
`RF_SIZES` as for the kernels; `RF_COUNTERS` picks up to four of `ins`, `uops`, `dec`, `brmiss`
or raw AMD events `rXXXX`; `RF_MIXED=lo,hi[,period]` sets the mixed loop's sizes. `compare.sh`
reads the baseline libraries from the main checkout's `bench/baselines/out` (built by
`bench/baselines/build.sh`); `RF_ALLOCS` and `RF_BUILDS` narrow it. The stand-in allocator is
single-threaded and never returns memory; it exists only to put the routines behind `malloc`.

Linking a C program with the static library (`static/`, or any crate that invokes
`export_c_abi!` as a `staticlib`) against static glibc with lld needs `-lgcc` before the library:

```
cc -static -fuse-ld=lld prog.c -lgcc libcentry_static.a
```

The library carries Rust's `compiler_builtins`, prebuilt with unwind tables that name
`rust_eh_personality`, which a `no_std` library does not define. GNU ld takes the soft-float
routines static glibc needs (`__multf3`, `__addtf3`, ...) from `libgcc.a`, which follows `libc.a`;
lld takes them from the first archive on the line that defines them, ours, and then needs the
personality. `-Wl,--gc-sections` does not help. Dynamic links do not need those routines.

## machine

```
cargo build --release --manifest-path bench/roofline/machine/Cargo.toml
cargo build --release --manifest-path bench/roofline/oscost/Cargo.toml --bin faults
bench/roofline/machine/run.sh OUT            # the whole bill; about ten minutes on a quiet machine
bench/roofline/machine/run.sh OUT streams    # one group: lines moves hier streams kernel huge clock
target/release/topo                          # which CPUs it would use, and why
```

Nothing names this machine. Page and huge-page sizes come from the kernel, the timer's rate from
`cntfrq_el0`, the PMU from sysfs (a device with a `cpus` file and a cycle event), and CPU choices
from `topo`: a CPU of the fastest kind, a neighbour in its last-level cache, one in another
domain or else the farthest by number, and a CPU of the slowest kind if there are two kinds. Where
a virtual machine gives no counters the probes read nanoseconds only, and `cycles` columns read 0.
`RF_LINE` sets the spacing probes lay lines out at (default: sysfs, or 128), `RF_MAX_MIB` caps the
sweeps for a small virtual machine, and each binary's header gives its commands.

| Binary | Commands | Answers |
|---|---|---|
| `lines` | `regs`, `granule A B`, `neighbour CPU` | What the registers say, the real coherence unit between two CPUs, whether a neighbour comes with a miss |
| `moves` | `pingpong`, `atomics`, `nextuser`, `nextread` `A B` | A line handed over, contended atomics, what the next user of an object pays, in a burst of stores and in a chase |
| `hier` | `latency`, `tlb`, `tlbsets`, `sets`, `unit`, `prefetch`, `run`, `bandwidth` | Latency by working set, TLB reach and sets, caches' sets, ways and index unit, what the prefetchers follow, bandwidth |
| `streams` | `stagger`, `rule`, `origin`, `sawtooth`, `pointers`, `magazine` | Blocks written in turn by stagger, the allocator's rule by class, class origins, magazine order, an array of pointers against offsets, and the same in pops of magazines |
| `kcost` | `purge`, `calls`, `barrier` | `MADV_DONTNEED` by length, alone and beside other threads; the rest of the family; `membarrier` |
| `worth` | `churn`, `build`, `cold`, `contig` | What a huge page is worth, what splitting one costs, whether the TLB uses the contiguous bit |
| `clockidle` | `ramp`, `load`, `wake` | The clock after idle and under load, a wake-up by idle gap and the first work after it |
| `topo` | | The CPU sets above, as shell variables and a table |

Results are in `docs/research/machine-aarch64.md`.

## rust-alloc

mimalloc is linked from a static library built from the local checkout (not committed):

```
mkdir -p /tmp/mi
cc -O3 -DNDEBUG -fPIC -I ../../../third_party/mimalloc/include \
   -c ../../../third_party/mimalloc/src/static.c -o /tmp/mi/mimalloc.o
ar rcs /tmp/mi/libmimalloc.a /tmp/mi/mimalloc.o
```

jemalloc is the system `libjemalloc.so` (Fedora: `jemalloc-devel`). Then:

```
cd rust-alloc
MI_LIB_DIR=/tmp/mi ./run.sh           # all allocators, release and fat-LTO profiles
MI_LIB_DIR=/tmp/mi ./run.sh x_        # only the cross-thread workloads
```

`run.sh` builds one binary per allocator (cargo feature `mi`, `mi_sized`, `je`, `floor_tls`,
`floor_heap`, `asm` for cralloc, `asm_exe` for cralloc with its `executable` feature, or none
for glibc) and profile, then runs them. mimalloc and jemalloc are called
the way the `mimalloc` and `tikv-jemallocator` crates call them. Without `MI_LIB_DIR` the
mimalloc variants are skipped.

Single-thread workloads (`pair_const`, `pair_box`, `pair_dyn`, `batch_const`, `batch_dyn`,
`pair_mix`, `batch_mix`, `churn`) report user-space cycles and instructions per allocate and free.
`_dyn` hides a fixed size from the compiler; `_mix` cycles through the sizes the server profile
found. Building with `RA_PAD=N` (a multiple of 16; loops start 16-aligned) moves each of their
loops by N bytes, to average a comparison over
code placements (`docs/research/roofline.md`, "The floor and where cralloc stands");
`RF_SINGLE_ONLY=1` skips the cross-thread workloads. Cross-thread workloads report
nanoseconds per object that each side spent working (waiting is excluded), and wall time:

| Workload | Shape |
|---|---|
| `x_local` | one thread allocates a burst, then frees it |
| `x_cross` | one thread allocates, another frees |
| `x_cross_touch` | the same, and the freeing thread reads each object first |
| `x_swap2`, `x_swap4` | 2 or 4 threads in a ring; each allocates for the next and frees what it receives |
| `x_cross_pairs` | `RF_PAIRS` independent allocate/free pairs at once |

`RF_CPU` and `RF_CPU2` (defaults 2 and 3) are the two CPUs; further threads use `RF_CPU+2`,
`RF_CPU2+2` and so on. `RF_BATCH` (default 32) is the burst size, `RF_PAIRS` (default 3) the
pair count, `RF_BUILD_CPUS` (default 4-7) where builds run. For the other L3 on the reference
machine: `RF_CPU2=10`.

## oscost

```
cd oscost && cargo run --release
```

Wall-clock, single run. `oscost` maps and touches 512 MiB. It assumes 4 KiB pages and x86-64.
`faults` takes its sizes from the kernel and its counters from the pinned CPU's PMU.
