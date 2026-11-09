# Follow-ups for a Graviton 4 host

Graviton 4 and Grace are Neoverse V2 cores. Everything we say about them comes from public sources and
reasoning; nothing has run on one. When a host exists, do these roughly in order. Delete an item when
it is done, and put what it found in the note on its topic.

## First, what we reasoned instead of measured

Check the distribution's glibc first: cralloc links `__rseq_offset`, which glibc added in 2.35, and we
have only run 2.43.

Then run the portable tools, which find the CPUs and PMU from sysfs:

- `bench/roofline/aarch64/` for the core and the routines, compared with the V2 column of
  `docs/research/firestorm.md`. Does V2 rename memory at all? Is any routine's shape bad there?
- `bench/roofline/machine/run.sh OUT` for the machine, compared with `docs/research/machine-aarch64.md`.
- `scripts/gate` and `scripts/cpusets`, whose fixture for this machine is all that has tested it.

## Questions only this machine can answer

- **Many cores.** Nothing has run on more than eight performance cores. What do magazines park on 96 or
  192 CPUs, how contended are depot stacks in one cache domain, and is `SIBLINGS` (64) too few workers?
- **A 64 KiB kernel,** if one is available. Huge pages are off at 512 MiB on numbers that are computed,
  so run `worth churn` and `worth cold`. The 64 KiB tests here are simulated only.
- **A 4 KiB kernel on bare metal.** Our 4 KiB runs are in a VM, with no transparent huge pages, so the
  2 MiB fault and split costs and the stagger's gain there are computed or inflated.
- **The page stagger:** V2's L2 TLB size and sets are unverified. If it is not indexed by low page bits,
  the page term costs an eighth of a slot's address space for nothing. `streams rule` shows it.
- **Inlining in a large program.** On Firestorm, whose L1I is 192 KiB against V2's 64, an out-of-line
  build took 0.5 to 1.7% off the cycles of six jobs and added 4% to `tree depth=18` (`firestorm.md`,
  "What inlining costs a program"). Repeat it: `pad-build.sh` builds `cro` beside `cra`, with
  `BENCH_PROCESS_EVENTS` set to this PMU's L1I and iTLB events.
- **The pointer prefetch.** Magazines of raw addresses gain on Firestorm, and V2 reports an indirect
  prefetcher: `streams pointers` and `magazine`.
- **`membarrier` in a VM:** 49 ms in a microVM on a busy host, against 6.3 us on this one, and a drain
  waits for it. Does a cloud instance with dedicated vCPUs see this?
- **The timer:** the rounding that keeps an interval near a second is computed for a rate near 1 GHz.
- **SVE across a system call.** The M1 has no SVE, so nothing has run with SVE state live. `syscall`
  declares what the kernel discards (`docs/safety/cralloc/os/linux/syscall-safety.md`, item 3), and
  only the compiler's output says that this is enough. Build for `neoverse-v2` and run the gate, then
  write a test that keeps a predicate and a long vector live across a call.
- **Memory order:** the ordering tests on another core (`scripts/measure --both-l3`). A pass here says
  little on V2, so this is worth doing even though the arguments rest on Arm's model.
