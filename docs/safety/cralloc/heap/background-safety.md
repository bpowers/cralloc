# Safety ledger: `crates/cralloc/src/heap/background.rs`

The background thread's creation through the C library (feature `background`).

## BACKGROUND-01: the declarations of the C library's functions

**Obligation.** Each declaration matches the definition the linker binds it to, in name, ABI,
argument types and return type, and the `link` attribute names a library that defines them.

**Proof.** The C prototypes (POSIX and glibc's `pthread.h`, `signal.h`) against the Rust
declarations, on x86-64 System V, where every pointer is 8 bytes in a general register and
`int` is `c_int`:

- `int pthread_create(pthread_t *, const pthread_attr_t *, void *(*)(void *), void *)`:
  `pthread_t` is `unsigned long` in glibc and a pointer in musl, 8 bytes either way, so
  `*mut usize` points at room of the right size and alignment. The attribute is passed as a
  null untyped pointer, the defaults. The start routine is `extern "C"`, one pointer in, one
  out. `CreateThread` is the same type, for the preload library's handle on the next
  `pthread_create`.
- `int pthread_detach(pthread_t)`: the identifier by value as `usize`.
- `int sigfillset(sigset_t *)` and `int pthread_sigmask(int, const sigset_t *, sigset_t *)`:
  `SigSet` is `#[repr(C)]` over `[u64; 16]`, 128 bytes aligned to 8, which is `sigset_t` in
  glibc (`unsigned long __val[16]`) and musl alike. `SIG_BLOCK` is 0 and `SIG_SETMASK` 2 on
  Linux x86-64.
- `int sigaltstack(const stack_t *, stack_t *)`: `SignalStack` is `#[repr(C)]` `{ *mut
  c_void, c_int, usize }`, which is x86-64's `stack_t` `{ void *ss_sp; int ss_flags; size_t
  ss_size; }` with the same padding after the int. `SS_ONSTACK` is 1.

`#[link(name = "c")]` adds `-lc`. glibc 2.34 and later define every one of these in
`libc.so.6`, and musl's `libc.so` too. A Rust program that links `std` links the C library
already; a `no_std` program that keeps the default feature now links it too, which the
feature's documentation says.

## BACKGROUND-02: `choice`, which reads the environment without the C library

There is no `unsafe` here; the entry records why the read needs none, since it replaced one
that was not sound.

**What was wrong.** `choice` called `getenv` from a slow path, in any thread, at any time. C
lets another thread's `setenv` or `putenv` replace the environment meanwhile, and musl frees
the old array and strings when it does, so the read could be a use after free. A program can
order its own `getenv` against its own `setenv`, but not one an allocator makes behind it.

**What it does now.** `crate::environment` reads `/proc/self/environ` with the crate's own
system calls (`LinuxOs::environment`, whose open, reads and close are KERNEL-10) into a
buffer on the stack, and keeps the answer in one word of the depot (`Common::switch`).

- The kernel's copy is the memory between the process's `env_start` and `env_end`, read
  through the kernel, so no pointer into the C library's `environ` array is followed, and a
  concurrent `setenv` cannot free anything the read touches. If the program writes over that
  area meanwhile (a process title), the read sees some mix of old and new bytes: still bytes,
  of which the scan only compares and counts. The scan cannot read past what a read returned.
- The word is written only by a compare-and-swap from zero, with a value from 1 to 3, so it
  holds zero or one value for the rest of the process. Racing readers each compute an answer;
  the first stored is everyone's. Nothing rests on the answer but whether a thread is created.
- It is read at the process's first slow path (`generation::prepare_first`), before a
  program can enter a seccomp sandbox that would kill an `openat`, and in the preload library
  at the latest by its constructor, before `main`.
- A forked child inherits the word with its parent's memory, so it neither reads nor
  disagrees: its own initial environment area is a copy of its parent's, the same bytes the
  parent read (or later bytes, if the parent wrote a title over them, which is why the child
  must not read again). A process started by `exec` has a fresh, zero word and reads its own.
- No `/proc`, a refused open or a failed read gives the default. The open is refused
  (`EACCES`) to a process that is not dumpable when it reads, which a setuid binary of a user
  other than root, a setgid binary or one with file capabilities is from its start: for such
  a process the variable is ignored, as `secure_getenv` ignores its environment. A process
  that makes itself non-dumpable after its first slow path keeps what it read (measured: an
  open after `prctl(PR_SET_DUMPABLE, 0)` fails with `EACCES`; the read has happened by then).

## BACKGROUND-03: `spawn` and the calls to it

**Preconditions.** `create` has `pthread_create`'s contract. The masks passed are valid for the
calls. The new thread runs `run` with a null argument, which it ignores, and `run` never
returns or unwinds; its stack holds its deepest frames.

**Invariants.**

- I1. `start_wanted` and `start_background` pass the C library's own `pthread_create`;
  `start_dormant` passes what its caller promises is one (the preload library passes the next
  `pthread_create` found by `dlsym(RTLD_NEXT)`, PRELOAD-05).
- I2. `all` and `old` are locals that outlive the calls that use them; the attribute pointer
  is null, which `pthread_create` takes as the defaults.
- I3. `pthread_sigmask(SIG_BLOCK, &all, &mut old)` after `sigfillset(&mut all)` blocks every
  signal the C library lets a program block, which leaves out its internal signals (glibc's
  `SIGCANCEL` and `SIGSETXID`), so that a change of credentials still reaches the new thread.
  The new thread inherits the mask in force when it is created; the caller's own mask is put
  back with `SIG_SETMASK` whatever `create` returned.
- I4. `run` calls `Bound::run_thread`, which loops for ever and does not unwind: every call in
  it is the allocator's, which does not panic while serving (the crate's rule), and the
  library is built with `panic = "abort"` where it matters (the preload library); a Rust
  program's unwinding panic cannot start in allocator code.
- I5. `pthread_detach` is called only after `create` returned 0, with the identifier it
  stored. The thread names itself at its start with the crate's own `prctl` (KERNEL-13),
  which opens no file: the C library's `pthread_setname_np` on another thread opens
  `/proc/self/task/<tid>/comm`, which a sandbox entered after start may forbid.
- I8. `errno` is put back after the calls: `spawn` saves it first (ERRNO-02) and restores it
  whatever the calls did, so the allocation or free that started the thread changes no
  `errno`, as glibc's `free` changes none.
- I6. The stack holds the thread. It is the C library's default, the one the program's own
  threads get, which the library sizes against its own thread-local data
  (`docs/research/portability.md`, "The background thread's stack"). The thread's frames reach
  about 8 KiB in a release build and 34 KiB in a debug one (*measured*). glibc's default
  (*measured*, and read in the 2.43 source by the reviewer) is the soft `RLIMIT_STACK` fixed at
  start, 2 MiB if it is unlimited and 16 KiB if it is below that, raised to at least a page plus
  the static thread-local block plus 2048 bytes, so the frames get the limit less the static
  block, and at least about 6 to 10 KiB: 250,000 bytes of thread-local data under
  `ulimit -s 256` or `-s 128` left the thread 9,776 bytes, and 104 bytes under `ulimit -s 16`
  left 11.6 KiB. musl's default (*read in source* and *measured*) is 128 KiB plus the
  thread-local data and its own block. A limit: a process started with a stack limit below its
  static thread-local data plus about 40 KiB, or a program that lowers the default with
  `pthread_setattr_default_np`, gives this thread the same few KiB as its own default threads,
  which a release build's rounds fit and a debug build's may not. Another: a program that
  locks all its memory (`mlockall(MCL_FUTURE)`) gets the whole default stack resident, 8 MiB
  under the usual limit, as for each of its own threads. Without that, only the pages the
  thread touches are resident, and the kernel keeps huge pages off a thread stack (`nh` in
  its `VmFlags`, *measured*).

**Proof.** By I1 the call creates an ordinary thread of the C library. By I2 every pointer
passed is valid for what the call does with it. By I3 the new thread has every program signal
blocked, so a handler never runs on it and process-directed signals go to the program's
threads, while the caller's mask is unchanged after the call. By I4 the thread never returns
into the C library with a value it would interpret, and never unwinds through an `extern "C"`
frame. By I5 the detach is made on a live thread, and naming needs no file. By I8 the
caller's `errno` is unchanged. By I6 the thread's frames fit its
stack, within the stated limit. A failure of `create` returns its error, leaves the caller's
mask as it was, and starts nothing.

`start_dormant`'s call passes its own `create` through: its caller promises what `spawn`
asks (`# Safety` of both is the same sentence), and nothing else is added.

Locks: `spawn` takes no lock of its own and calls nothing of the C library but
`pthread_create`, `pthread_detach`, the signal calls (`sigfillset`, `pthread_sigmask`, and
`sigaltstack` in the test before it) and `__errno_location`, none of which but
`pthread_create` takes a lock; the naming is the crate's own system call. So a fork child, whose copy of a lock may be
held by a thread that does not exist there, can start the thread whenever its program could
create one of its own (`tests/fork_loader_lock.rs`). A limit remains, the same as for any
`pthread_create`: glibc's `pthread_create` takes `dl_load_tls_lock`
(`_dl_allocate_tls_init`), and `dlopen` and `dlclose` hold `dl_load_tls_lock` and then take
`dl_load_write_lock` (`dl_open_worker` around `_dl_add_to_namespace_list`;
`_dl_close_worker`), the opposite order, so the slow path that starts the
thread, once per process, deadlocks if it runs inside a `dl_iterate_phdr` callback, which
holds `dl_load_write_lock` (std's backtrace symbolizer allocates in one), while another
thread is inside `dlopen` or `dlclose` (reproduced by the reviewer; the case
`a_free_in_a_callback_returns_beside_a_dlopen`, run with `CRALLOC_TEST_KNOWN_FAILURES=1`).
No cheap and sound test tells a slow path that its thread holds the loader's lock: the lock
is private to glibc. On musl, `dl_iterate_phdr` holds no lock while its callback runs (read in
source: it takes its read lock only to step to the next module).

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

Reviewed: 7bad17e
