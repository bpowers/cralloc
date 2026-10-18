# Safety ledger: `crates/asmalloc-preload/src/background.rs`

The preload library's background thread: a constructor that reads `ASMALLOC_BACKGROUND` and,
if asked, starts a thread with the C library's `pthread_create`, with every signal blocked in
it, that sleeps with `nanosleep` and calls `asmalloc::Asmalloc::maintain`, forever. These are
the library's references to the C library besides the allocator's `__errno_location`
(ERRNO-01, `docs/safety/asmalloc/c/errno-safety.md`).

## PRELOAD-03: the declarations of `getenv`, `sigfillset`, `pthread_sigmask`, `pthread_create` and `nanosleep`

**Obligation.** Each declaration matches the definition the linker binds it to, in name, ABI,
argument types and return type, and the `link` attribute names a library that defines them.

**Proof.** The C prototypes (POSIX; glibc's `stdlib.h`, `signal.h`, `pthread.h`, `time.h`),
against the Rust declarations, on x86-64 System V, where every pointer is 8 bytes passed and
returned in a general register and `int` is `c_int` (`i32`):

- `char *getenv(const char *)`: `fn(*const c_char) -> *const c_char`. The constness of the
  returned pointer is not part of the ABI; the library never writes through it.
- `int sigfillset(sigset_t *)` and `int pthread_sigmask(int, const sigset_t *, sigset_t *)`:
  `SigSet` stands for `sigset_t`. glibc's is `struct { unsigned long __val[1024 / (8 *
  sizeof (unsigned long))]; }`, sixteen 8-byte words, 128 bytes aligned to 8; musl's is
  `struct { unsigned long __bits[128 / sizeof (long)]; }`, the same. `SigSet` is
  `#[repr(C)]` over `[u64; 16]`: 128 bytes, aligned to 8. Only pointers to it cross the call.
  `SIG_BLOCK` is 0 and `SIG_SETMASK` is 2 on Linux (`asm-generic/signal-defs.h`, which both
  C libraries follow on x86-64).
- `int pthread_create(pthread_t *, const pthread_attr_t *, void *(*)(void *), void *)`:
  `fn(*mut usize, *const c_void, extern "C" fn(*mut c_void) -> *mut c_void, *mut c_void) ->
  c_int`. `pthread_t` is `unsigned long` in glibc and a pointer in musl, 8 bytes either way,
  so `*mut usize` points at room of the right size and alignment for the identifier the call
  stores. The attribute pointer is only ever null. The start routine's type is the C one:
  the C calling convention, one pointer in, one pointer out.
- `int nanosleep(const struct timespec *, struct timespec *)`: `Timespec` is `#[repr(C)]`
  `{ tv_sec: c_long, tv_nsec: c_long }`; x86-64's `struct timespec` is `{ time_t tv_sec; long
  tv_nsec; }` with `time_t` a `long`, in glibc and musl alike.

`#[link(name = "c")]` adds `-lc`. glibc 2.34 and later define all five in `libc.so.6`
(`pthread_create` and `pthread_sigmask` moved there from `libpthread`), and the design's
glibc is 2.43; musl's single `libc.so` defines them too. The library already needs `libc.so.6`
(ERRNO-01), so this adds no dependency.

## PRELOAD-04: `#[used] #[unsafe(link_section = ".init_array")] static CONSTRUCTOR`

**Obligations.** An entry of `.init_array` is a pointer to a function the dynamic loader may
call with the C calling convention; the function is sound to run when the loader runs it.

**Proof.** The static is one `extern "C" fn()`, a code pointer of 8 bytes, placed alone in the
library's `.init_array`, which the linker merges with the section's other entries and records
in the dynamic section as `DT_INIT_ARRAY`. `#[used]` keeps it through LTO and the linker's
garbage collection. In a position-independent library the entry needs a relative relocation,
which the loader applies before it calls any initializer. glibc calls each entry as `void
(*)(int, char **, char **)` and musl with no arguments; a function of no arguments called with
three in registers ignores them, since under System V the caller owns the argument registers
and the stack.

The loader calls initializers once the library and everything it depends on are relocated,
dependencies first: `libc.so.6` is initialized (and its early initialization, which makes
`pthread_create` usable, is done) before this library's constructor runs. The constructor
reads the environment (PRELOAD-05) and may start the thread (PRELOAD-06). What that touches of
the allocator is sound at any moment of the process's life: the heap needs no initialization
(an all-zero heap is a valid one), the library's C exports are bound by relocation before any
initializer runs (PRELOAD-01, "Before the library can be called"), and glibc has registered
rseq for the main thread by then. `pthread_create` itself allocates from the heap (the new
thread's TLS vector), through the exported `malloc` and `calloc`, which is the ordinary use of
them. The function does not unwind: nothing it calls throws, and the library is built with
`panic = "abort"`.

## PRELOAD-05: `getenv(VARIABLE.as_ptr())` and `CStr::from_ptr` of its result, in `requested`

**Preconditions.** The argument is a NUL-terminated string. The result, if not null, points
at a NUL-terminated string that stays valid and unchanged while it is read.

**Invariants.**

- I1. `VARIABLE` is a `&'static CStr`, so `as_ptr` gives a NUL-terminated string that lives
  for the program.
- I2. `getenv` returns null or a pointer into the environment: a NUL-terminated string that
  stays valid until the environment is changed (`setenv`, `putenv`, `unsetenv`, or a write to
  `environ`). Neither C library allocates in `getenv`.
- I3. Precondition, C's own rule for `getenv`: nothing changes the environment while the
  call runs and its result is read. The constructor runs when the loader loads the library,
  before `main`. Loaded by `LD_PRELOAD`, the only threads that can exist then are ones the
  constructors of libraries initialized before this one started, which is rare and which
  would have to change the environment at that moment to break the rule. Loaded by `dlopen`
  (not the library's use; it would not take over `malloc` there), the program's other
  threads must not change the environment meanwhile, as for any library constructor that
  reads it.

**Proof.** By I1 the argument is valid. By I2 and I3 a non-null result is a NUL-terminated
string that does not change before `to_bytes` has read it and the comparison is done; the
borrow ends inside `requested`. `CStr::from_ptr`'s requirements (valid for reads up to and
including the NUL, not mutated for the lifetime of the borrow) are exactly these.

## PRELOAD-06: `sigfillset`, `pthread_sigmask` and `pthread_create` in `spawn`

**Preconditions.** Every pointer passed is valid for what the call reads or writes; the new
thread's start routine is sound to run on a new thread with the argument given, and does not
return in a way the C library cannot handle.

**Invariants.**

- I1. `all`, `old` and `thread` are locals of `spawn`, live for every call in the block, each
  of the size and alignment its C type needs (PRELOAD-03). `sigfillset` writes `all`;
  `pthread_sigmask` reads `all` and writes `old` (glibc writes the 8 bytes the kernel's mask
  has, musl as much; never more than `sigset_t`); `pthread_create` writes `thread`; the last
  `pthread_sigmask` reads `old` and is given a null old-mask pointer, which it accepts.
- I2. The attributes are null, which means the defaults: a joinable thread with the default
  stack. It is never joined; it never exits, so the lack of a join leaks nothing.
- I3. The argument is null, and `maintain_forever` ignores it, so the new thread reads nothing
  of `spawn`'s frame, which may be gone before the thread runs.
- I4. `maintain_forever` loops for ever on `nanosleep` (PRELOAD-07) and
  `asmalloc::Asmalloc::maintain`, a safe function that never waits and never panics while
  serving a request, and holds no lock. It never returns, so its return value is never
  read. It does not unwind: the library is built with `panic = "abort"`, and `nanosleep`,
  a cancellation point, never sees a cancellation, since the thread's identifier never leaves
  `spawn` and so nothing can name it to `pthread_cancel`.
- I5. The thread is the C library's own: it has its TLS and, before its start routine runs,
  its rseq registration (design: "What is trusted"). `maintain` uses neither the routines nor
  the thread's registration, but whatever it calls in the allocator may run on any thread.
- I6. A new thread starts with its creator's signal mask (POSIX), here every signal
  `sigfillset` names. glibc's `sigfillset` leaves out its internal signals (`SIGCANCEL` and
  `SIGSETXID`) and musl's its own, so the C library's cross-thread operations (`setuid` and
  the like) still reach the thread; SIGKILL and SIGSTOP cannot be blocked. If the first
  `pthread_sigmask` fails the function returns before creating anything; whether or not
  `pthread_create` succeeds, the second `pthread_sigmask` puts the caller's own mask back.

**Proof.** By I1 every call gets valid pointers of the right types for what it reads and
writes. By I2 and I3 the new thread needs nothing from `spawn`, and by I4 and I5 its start
routine is sound on it. The creator's mask is changed only between the two `pthread_sigmask`
calls (I6); a signal that arrives meanwhile stays pending and is delivered when the mask is
restored, which is ordinary signal semantics and no memory-safety question. So the block makes
only valid calls and leaves the calling thread as it found it, with one more thread in the
process.

## PRELOAD-07: `nanosleep(&period, ptr::null_mut())` in `maintain_forever`

**Preconditions.** The request is a valid `timespec` (a non-negative `tv_nsec` below one
billion); the remainder pointer is null or valid for a write.

**Proof.** `period` is a local that outlives the call, built from a `Duration`
(`Asmalloc::MAINTAIN_EVERY`, half a second): its whole seconds fit `c_long`, and
`subsec_nanos` is below one billion by `Duration`'s invariant. A null remainder is allowed and tells the call not to report the rest. The call
writes nothing else. It returns early only if a signal interrupts it, and every signal that
could is blocked in this thread (PRELOAD-06, I6), apart from the C library's internal ones;
an early return only runs `maintain` early, which is safe at any time.
