# Roofline harness

Small, dependency-free programs that measure what an allocator fast path can cost on the
host, and what the kernel and the Rust toolchain add around it. The results and what they
mean are written up in `docs/research/zen5-roofline.md`.

Each directory is a stand-alone cargo project (none belongs to a workspace). Everything is
x86-64 Linux only and needs `perf_event_open` for user-space counters
(`/proc/sys/kernel/perf_event_paranoid` of 2 or lower).

Run measurements on a quiet machine. Programs pin themselves; builds should be kept off the
measurement CPUs (`taskset -c 4-7 cargo build ...`). Defaults assume the reference machine,
where CPUs 2 and 3 are two cores of one L3, 18 is the SMT sibling of 2, and 10 is in the other
L3; override with the environment variables below.

| Directory | Measures | Run |
|---|---|---|
| `kernels/` | Hand-written assembly fast paths, store-to-load forwarding, atomics, call cost, core widths, and a code-layout sweep. Cycles, instructions, retired uops and forwards per operation. | `cargo run --release -- [prefix,...]` |
| `rust-alloc/` | Allocation workloads through Rust's global allocator for glibc, mimalloc, jemalloc and two floor allocators, single-threaded and cross-thread. | `./run.sh [workload-prefix]` |
| `inline/` | Whether the allocator fast path is inlined at different kinds of call site, per cargo profile. | `./check.py [variant ...]` |
| `oscost/` | Cost of mmap, page faults, the madvise family, mprotect, guard pages and zeroing. | `cargo run --release` |
| `xcore/` | Cost of moving a cache line between cores. | `cargo run --release` |
| `toolchain/` | The relocation a direct load of glibc's `__rseq_offset` gets, in an executable and a shared library, and the code rustc generates around an `asm!` block that calls. | `./check.sh` |

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
`lean/generated/`, instruction for instruction, and variants that each change one thing: their
placement in a cache line, their encoding (short jumps, registers, address arithmetic, where the
mask table sits), the CPU-number test, how `__rseq_offset` is read, per-class entries, and an
inline pop that calls its slow path. The script's docstring names every variant.

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
`floor_heap`, or none for glibc) and profile, then runs them. mimalloc and jemalloc are called
the way the `mimalloc` and `tikv-jemallocator` crates call them. Without `MI_LIB_DIR` the
mimalloc variants are skipped.

Single-thread workloads (`pair_const`, `pair_box`, `pair_dyn`, `batch_const`, `churn`) report
user-space cycles and instructions per operation. Cross-thread workloads report nanoseconds per
object that each side spent working (waiting is excluded), and wall time:

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

## inline

```
cd inline
taskset -c 4-7 ./check.py              # every variant and profile, about two minutes
taskset -c 4-7 ./check.py asm          # one variant
```

Builds a four-crate workspace (`fastalloc` the allocator, `dep` a dependency with allocation
sites, `galloc` a crate holding only the `#[global_allocator]` declaration, `app` the binary) in
six profiles and prints a table: per call site, whether the fast path was inlined (judged from
the disassembly), whether the call to `__rust_no_alloc_shim_is_unstable_v2` remains, and cycles
per alloc+free pair. Variants: `rust` (plain Rust fast path), `asm` (an `asm!` rseq critical
section), `rust+used` and `asm+used` (the binary crate has a private `#[used]` static),
`asm+modasm` (it has module-level assembly with a local symbol), `asm+used+sep` (the allocator
is declared in `galloc` instead of in the binary crate). `fastalloc` is a stand-in with real
fast paths and throwaway slow paths; it is not a usable allocator.

## oscost and xcore

```
cd oscost && cargo run --release
cd xcore && cargo run --release
```

Wall-clock, single run. `oscost` maps and touches 512 MiB. `xcore` uses `RF_CPU` (2), `RF_SMT`
(18), `RF_NEAR` (3) and `RF_FAR` (10).
