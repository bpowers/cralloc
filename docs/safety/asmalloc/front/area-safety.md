# Safety ledger: `crates/asmalloc/src/front/area.rs`

The calling thread's rseq area on musl: the crate's own, generated from Lean
(`VG.X86_64.ownRseqArea`, `front/generated/x86_64/area.rs`), registered by the first slow path
of each thread and unregistered by a pthread key's destructor as the thread ends. Compiled on
musl only. The tests in the file run each path on the real kernel and C library: registration,
the page offset in two threads, 3000 detached threads, a destructor that frees after ours,
a thread that never allocates, a refused registration (seccomp), and a fork from a threaded
process.

References: musl 1.2.5 (`src/thread/pthread_create.c`, `pthread_key_create.c`,
`src/env/__init_tls.c`), rseq(2), `include/uapi/linux/rseq.h`.

**What the generated data gives.** A 32-byte `struct rseq` in `.tdata`, 32-byte aligned (it is
768 bytes into a 4096-aligned section), with `cpu_id` -1, so every thread's copy starts reading
-1; and a read-only word, `asmalloc_rseq_offset`, holding the area's offset from the thread
pointer as the linker computes it (`@tpoff`), which `front::__rseq_offset` names (FRONT-01).
musl copies the initialized TLS image into every thread's block and sets the thread pointer so
that a TLS symbol is at the thread pointer plus its `@tpoff` (`__copy_tls`), so
`thread pointer + __rseq_offset` is the calling thread's area.

## AREA-01: the declarations of `pthread_key_create`, `pthread_key_delete`, `pthread_setspecific`

**Obligations.** The signatures are the C library's.

**Proof.** POSIX declares `int pthread_key_create(pthread_key_t *, void (*)(void *))`,
`int pthread_key_delete(pthread_key_t)` and `int pthread_setspecific(pthread_key_t, const void
*)`. musl's `pthread_key_t` is `unsigned` (`alltypes.h`), `u32` on x86-64; a function pointer
that may be null is `Option<unsafe extern "C" fn(*mut c_void)>`; `int` is `c_int`. They are
declared only on musl, which defines all three in `libc.a`.

## AREA-02: `cpu_id(area)`, and the stores of `RETIRED` through it

**Obligations.** `area + 4` is valid for 4-byte atomic access for as long as the reference is
used, 4-aligned, and every other access to it is atomic or the kernel's.

**Proof.** `area` is the calling thread's area: from `area()` (the thread pointer, RSEQ-01,
plus the offset word, as above) or the key's value for the exiting thread, which `register`
set to the same address. The area is in the thread's TLS block, which musl frees only after the
thread's destructors have run, and the reference never leaves the function that made it.
The area is 32-aligned, so `cpu_id` at 4 is 4-aligned. The routines read it with a plain
32-bit load from assembly, and the kernel writes it while the area is registered; neither is a
Rust access. The stores of `RETIRED` are made only while the area is not registered (`retire`'s
contract): after a failed registration, when no key could be made (before registering), and in
the destructor after a successful unregistration. So the kernel never writes it concurrently,
and a write by user space to `cpu_id` of a registered area (which a V2 registration would punish
with SIGSEGV) never happens.

## AREA-03: the pthread key calls

**Obligations.** `pthread_key_create` gets a valid out pointer and a destructor that may run on
any exiting thread with a value; `pthread_key_delete` gets a key no thread uses;
`pthread_setspecific` gets a key the process made.

**Proof.** The out pointer is a live local. The destructor is `unregister_at_exit`, whose
contract (AREA-05) is exactly how musl calls it. A key is deleted only when this thread made
it, lost the race to publish it, and gave it no value, so no thread has one. `setspecific` is
called with the published key, which is never deleted. None of the three allocates in musl;
`pthread_key_create` and `pthread_key_delete` take musl's key lock, which only those two and the
exit path's destructor loop take, and they are called once per process; `setspecific` only
stores into the thread's own table.

## AREA-04: `rseq::register(area, …)` in `register`

**Obligations** (RSEQ-02). The area is the calling thread's, 32-aligned, 32 bytes, laid out as
`struct rseq`, valid until unregistered or the thread ends; every descriptor `rseq_cs` may name
stays mapped.

**Proof.** The area and its layout are the generated data's (above). It stays valid until the
thread ends or unregisters: the key's value is set before registering, so if the thread ends by
`pthread_exit` musl runs `unregister_at_exit` before freeing the TLS block (`__pthread_exit`
runs `__pthread_tsd_run_dtors` before `__unmapself` or the joiner's `munmap`). A thread that
ends by the process ending frees nothing first. A thread that ends by a raw `SYS_exit` is not a
musl thread and never calls the allocator through musl's thread start. The descriptors are in the
executable's `__rseq_cs` section, mapped for the life of the process (a musl build is an
executable; musl never unmaps a loaded object).

**Signal handlers.** A handler that interrupts `register` between the slow path's read of -1
and its system call, and allocates, registers the thread itself; the slow path's call then
returns `EBUSY`, which counts as registered, and its value for the key is the same area. That
window is covered. The window between a routine's arming store and its read of `cpu_id`, on the
fast path, is not; the next paragraph.

**What this does not cover: registration in the middle of a routine.** The obligations above
are about the slow path's call. The routines' own `# Safety` items require the thread to be
registered, or unregistered, for the whole of each call (`registeredDoc` in
`lean/Asmalloc/Spec/Slab.lean`), and a registration made by a signal handler during a routine
breaks that. The race between the slow path's read of -1 and its `register` is harmless (the
handler registers, the slow path gets `EBUSY`, which counts as registered). The race between a
routine's arming store and its read of `cpu_id` is not: on a thread at -1 the kernel does not
abort the interrupted routine, the handler's first allocation registers the thread, and the
routine then reads a valid CPU number with `rseq_cs` naming the handler's last sequence, so it
runs to its commit unprotected. No argument here rules that out; `docs/design.md` lists it
under "What is trusted", `front/area.rs` says when it can happen (a handler that allocates as
the thread's first allocator call, at that boundary, then a second event within about eight
instructions), and its ignored test `limitation_a_handler_that_registers_a_thread_inside_its_first_sequence_unprotects_it` shows it by single-stepping.

Residual: if a thread's first allocator call comes after its destructors' last round (musl runs
`PTHREAD_DESTRUCTOR_ITERATIONS`, 4), it registers and its value never gets a destructor run, so
a detached such thread could die registered. In musl that needs an allocation from the exit path
after the destructors (`__dl_thread_cleanup` frees a `dlerror` buffer only if the thread made
one, which needs an allocation first and so a registration and a value earlier).

## AREA-05: `unregister_at_exit`

**Obligations** (its `# Safety`). Called by the C library as the key's destructor, on the
thread whose area `value` is.

**Proof.** musl calls a key's destructor on the exiting thread itself, with that thread's value,
and only if it is non-null (`__pthread_tsd_run_dtors`). The value is the area's address, set by
that thread in `register`. Signals are blocked (RSEQ-04) around the unregistration and the
store of `RETIRED`, so no handler can find `cpu_id` -1 in between and register again; if
unregistering fails (it does not, as the arguments are the registration's), the area is left as
it is.
