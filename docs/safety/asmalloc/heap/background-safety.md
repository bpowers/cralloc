# Safety ledger: `crates/asmalloc/src/heap/background.rs`

The background thread's creation through the C library (feature `background`).

## BACKGROUND-01: the declarations of the C library's functions

**Obligation.** Each declaration matches the definition the linker binds it to, in name, ABI,
argument types and return type, and the `link` attribute names a library that defines them.

**Proof.** The C prototypes (POSIX and glibc's `pthread.h`, `signal.h`, `stdlib.h`) against
the Rust declarations, on x86-64 System V, where every pointer is 8 bytes in a general
register and `int` is `c_int`:

- `int pthread_create(pthread_t *, const pthread_attr_t *, void *(*)(void *), void *)`:
  `pthread_t` is `unsigned long` in glibc and a pointer in musl, 8 bytes either way, so
  `*mut usize` points at room of the right size and alignment. The attribute is passed as an
  untyped pointer to an `Attr` or null. The start routine is `extern "C"`, one pointer in, one
  out. `CreateThread` is the same type, for the preload library's handle on the next
  `pthread_create`.
- `int pthread_attr_init(pthread_attr_t *)`, `int pthread_attr_setstacksize(pthread_attr_t *,
  size_t)`, `int pthread_attr_destroy(pthread_attr_t *)`: `Attr` is `#[repr(C, align(8))]`
  over 56 bytes. glibc's `pthread_attr_t` on x86-64 is a union of `char[56]` and `long`, 56
  bytes aligned to 8; musl's is a union of int, long and pointer arrays, also 56 bytes aligned
  to 8. Only pointers to it cross the calls, and `size_t` is `usize`.
- `int pthread_detach(pthread_t)` and `int pthread_setname_np(pthread_t, const char *)`: the
  identifier by value as `usize`, and a NUL-terminated name.
- `int sigfillset(sigset_t *)` and `int pthread_sigmask(int, const sigset_t *, sigset_t *)`:
  `SigSet` is `#[repr(C)]` over `[u64; 16]`, 128 bytes aligned to 8, which is `sigset_t` in
  glibc (`unsigned long __val[16]`) and musl alike. `SIG_BLOCK` is 0 and `SIG_SETMASK` 2 on
  Linux x86-64.
- `int sigaltstack(const stack_t *, stack_t *)`: `SignalStack` is `#[repr(C)]` `{ *mut
  c_void, c_int, usize }`, which is x86-64's `stack_t` `{ void *ss_sp; int ss_flags; size_t
  ss_size; }` with the same padding after the int. `SS_ONSTACK` is 1.
- `char *getenv(const char *)`.

`#[link(name = "c")]` adds `-lc`. glibc 2.34 and later define every one of these in
`libc.so.6`, and musl's `libc.so` too. A Rust program that links `std` links the C library
already; a `no_std` program that keeps the default feature now links it too, which the
feature's documentation says.

## BACKGROUND-02: `getenv(VARIABLE.as_ptr())` and `CStr::from_ptr` of its result, in `choice`

**Preconditions.** The argument is NUL-terminated. The result, if not null, points at a
NUL-terminated string that stays valid and unchanged while it is read.

**Proof.** `VARIABLE` is a `&'static CStr`. `getenv` returns null or a pointer into the
environment, which neither C library allocates for. The string stays valid until the
environment changes; C requires that nothing change it while another thread calls `getenv`,
and the reads here are as exposed to that as any `getenv` in the program. The bytes are copied
out at once (`to_bytes` is compared and dropped before return).

## BACKGROUND-03: `spawn` and the calls to it

**Preconditions.** `create` has `pthread_create`'s contract. The masks and attribute passed
are valid for the calls. The new thread runs `run` with a null argument, which it ignores, and
`run` never returns or unwinds.

**Invariants.**

- I1. `start_wanted` and `start_background` pass the C library's own `pthread_create`;
  `start_dormant` passes what its caller promises is one (the preload library passes the next
  `pthread_create` found by `dlsym(RTLD_NEXT)`, PRELOAD-05).
- I2. `all`, `old` and `attr` are locals that outlive the calls that use them; `attr` is
  destroyed only if `pthread_attr_init` succeeded, and passed only then.
- I3. `pthread_sigmask(SIG_BLOCK, &all, &mut old)` after `sigfillset(&mut all)` blocks every
  signal the C library lets a program block, which leaves out its internal signals (glibc's
  `SIGCANCEL` and `SIGSETXID`), so that a change of credentials still reaches the new thread.
  The new thread inherits the mask in force when it is created; the caller's own mask is put
  back with `SIG_SETMASK` whatever `create` returned.
- I4. `run` calls `Bound::run_thread`, which loops for ever and does not unwind: every call in
  it is the allocator's, which does not panic while serving (the crate's rule), and the
  library is built with `panic = "abort"` where it matters (the preload library); a Rust
  program's unwinding panic cannot start in allocator code.
- I5. `pthread_setname_np` and `pthread_detach` are called only after `create` returned 0,
  with the identifier it stored; a name of 8 bytes and NUL is under the 16-byte limit.

**Proof.** By I1 the call creates an ordinary thread of the C library. By I2 every pointer
passed is valid for what the call does with it. By I3 the new thread has every program signal
blocked, so a handler never runs on it and process-directed signals go to the program's
threads, while the caller's mask is unchanged after the call. By I4 the thread never returns
into the C library with a value it would interpret, and never unwinds through an `extern "C"`
frame. By I5 the name and detach are made on a live thread. A failure of `create` returns its
error, leaves the caller's mask as it was, and starts nothing.

Re-entry: `pthread_create` allocates (the new thread's TLS and stack bookkeeping), which comes
back into the allocator through the C interface or the Rust one. The caller holds nothing
private across the call: a Rust-interface slow path calls the start hook after its own slow
path has returned (`Bound::after_rust_slow_path`), and the explicit and preload starts are
called from no slow path at all. The word moved off `WANTED` (or `NONE`) with a
compare-and-swap before the call, so a nested slow path that loads it does not start a second
thread.

## BACKGROUND-04: the signal queries in `signals_quiet`

**Preconditions.** A null new mask and a null new stack change nothing; the old-value
pointers are valid for writes of a `sigset_t` and a `stack_t`.

**Proof.** `pthread_sigmask(SIG_BLOCK, NULL, &mask)` only stores the current mask
(POSIX: with a null `set`, `how` is ignored); `sigaltstack(NULL, &stack)` only stores the
current alternate stack. Both targets are locals of the right layout (BACKGROUND-01).
