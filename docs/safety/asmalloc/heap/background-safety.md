# Safety ledger: `crates/asmalloc/src/heap/background.rs`

The background thread's creation through the C library (feature `background`).

## BACKGROUND-01: the declarations of the C library's functions

**Obligation.** Each declaration matches the definition the linker binds it to, in name, ABI,
argument types and return type, and the `link` attribute names a library that defines them.

**Proof.** The C prototypes (POSIX and glibc's `pthread.h`, `signal.h`) against the Rust
declarations, on x86-64 System V, where every pointer is 8 bytes in a general register and
`int` is `c_int`:

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

`#[link(name = "c")]` adds `-lc`. glibc 2.34 and later define every one of these in
`libc.so.6`, and musl's `libc.so` too. A Rust program that links `std` links the C library
already; a `no_std` program that keeps the default feature now links it too, which the
feature's documentation says. `dl_iterate_phdr` is BACKGROUND-05.

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
- No `/proc`, a refused open or a failed read gives the default.

## BACKGROUND-03: `spawn` and the calls to it

**Preconditions.** `create` has `pthread_create`'s contract. The masks and attribute passed
are valid for the calls. The new thread runs `run` with a null argument, which it ignores, and
`run` never returns or unwinds; its stack holds its deepest frames.

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
- I6. The stack holds the thread. It asks for `NEED + LIBRARY + static_tls()` bytes
  (`stack_size`). glibc carves its own block (the thread descriptor, the static TLS surplus,
  alignment) and every module's static TLS out of the size it is asked for, and refuses with
  `EINVAL` only when less than `MINIMAL_REST_STACK`, 2 KiB, would remain below them; so an
  accepted request leaves `stack_size - (static TLS + glibc's block)` for the frames, and
  nothing larger than 2 KiB is promised by acceptance alone. That was the defect of a fixed
  256 KiB request, reproduced in `docs/research/thread-stack.md`: a program with 252,000
  bytes of TLS got about 5 KiB and the thread died in its first round. Now `static_tls()` is
  at least glibc's static TLS from the modules, because it sums every loaded module's
  `PT_TLS` segment rounded up to its alignment (BACKGROUND-05), which counts each module that
  has static TLS (those loaded at start and the `dlopen`ed ones with initial-exec TLS, which
  the surplus holds) and some that have none. So the frames get at least `NEED + LIBRARY -
  glibc's block`; glibc's block is about 4.5 KiB with the default tunables (measured), so
  the frames get over 180 KiB against a deepest use of 34 KiB in a debug build and 8 KiB in a
  release one. A limit: a program that raises `glibc.rtld.optional_static_tls` past about
  64 KiB enlarges glibc's block past `LIBRARY`; between that and `LIBRARY + NEED - 34 KiB`
  the thread could again get too little, and above it the request is refused and the
  default is taken (below). musl maps the size asked for plus the TLS and its own block
  (`pthread_create.c`: `size = guard + ROUND(stacksize + libc.tls_size +
  __pthread_tsd_size)`), so there `static_tls()` is 0 and the frames get all of `NEED +
  LIBRARY`. The sum saturates rather than wrap; a size the library cannot map fails the
  call, which starts nothing.
- I7. A first `create` with the attribute that `EINVAL` refused created no thread and stored
  nothing that is used; the second `create` passes a null attribute, which `pthread_create`
  takes as the defaults, with the same `thread`, `run` and argument, under the same mask, and
  only after `attr` was destroyed, so nothing destroyed is passed. The default stack is the
  one the program's own threads get.

**Proof.** By I1 the call creates an ordinary thread of the C library. By I2 every pointer
passed is valid for what the call does with it. By I3 the new thread has every program signal
blocked, so a handler never runs on it and process-directed signals go to the program's
threads, while the caller's mask is unchanged after the call. By I4 the thread never returns
into the C library with a value it would interpret, and never unwinds through an `extern "C"`
frame. By I5 the name and detach are made on a live thread. By I6 the thread's frames fit
its stack. By I7 the retry is a second ordinary call with valid arguments, made only after a
failure that created nothing. A failure of `create`, or of both, returns its error, leaves the
caller's mask as it was, and starts nothing.

`start_dormant`'s call passes its own `create` through: its caller promises what `spawn`
asks (`# Safety` of both is the same sentence), and nothing else is added.

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

## BACKGROUND-05: `dl_iterate_phdr` and its callback `add_tls` (glibc only)

**Blocks.** The declaration of `int dl_iterate_phdr(int (*)(struct dl_phdr_info *, size_t,
void *), void *)`; the call in `static_tls`; in `add_tls`, the references made from `info`
and `data`, and each `&*info.phdr.add(i)`.

**Preconditions.** The declaration matches glibc's. The callback's pointers are valid for
the reads made through them while it runs; `data` points at the `usize` the call passed. The
callback does not unwind, allocate or take a lock that the iteration's lock could be ordered
against. The call cannot deadlock where `spawn` is called.

**Proof.** Declaration: glibc (`link.h`) declares it so, in `libc.so.6` since 2.34 and in the
static library; `EachModule` is `extern "C" fn(*mut ModuleInfo, usize, *mut c_void) ->
c_int`, the callback's C type with the info pointer typed. `ModuleInfo` is
`#[repr(C)]` over the first four fields of `struct dl_phdr_info` (`ElfW(Addr) dlpi_addr;
const char *dlpi_name; const ElfW(Phdr) *dlpi_phdr; ElfW(Half) dlpi_phnum;`), 8, 8, 8 and 2
bytes at offsets 0, 8, 16 and 24, which is how glibc lays them out; the callback reads them
only when `size`, the size of the structure glibc passes, is at least `size_of::<ModuleInfo>()`
(32). `ProgramHeader` is `#[repr(C)]` `Elf64_Phdr` (two `u32`, six `u64`, 56 bytes).

Callback: glibc calls it with a pointer to a structure it filled for one module and the
`data` argument of the call, unchanged, in the calling thread, before `dl_iterate_phdr`
returns. `data` is `&mut sum`, a local of `static_tls` that outlives the call and that nothing
else refers to meanwhile, so the `&mut usize` is unique. `dlpi_phdr` points at the module's
`dlpi_phnum` program headers, mapped for the module's life, which the iteration's lock keeps
loaded while the callback runs; `i < phnum`, so each `add(i)` stays inside them. Null
pointers are checked first. The callback does integer arithmetic only, with `checked_` and
`saturating_` operations, so it cannot panic, and calls nothing.

Locks: glibc's `dl_iterate_phdr` holds `dl_load_write_lock`, a recursive lock, while it
iterates. That lock is otherwise held only for the short updates of the list of loaded
objects (within `dlopen` and `dlclose`, which take `dl_load_lock` first) and by other
`dl_iterate_phdr` calls while their callbacks run. So: a thread that already holds it (a
program's callback that allocates through the Rust interface and so may start the thread)
takes it again recursively; a thread that holds `dl_load_lock` (a constructor run by `dlopen`,
the case "What remains" in the module's documentation names) takes the two in glibc's own
order. The caller holds no allocator state and no lock of the allocator's (there are none),
so a thread that holds `dl_load_write_lock` waits for nothing the caller has. The three
callers are `start_wanted` (after a Rust slow path, with no signal blocked and not on a
signal stack), `start_background` (the program's own call) and `start_dormant` (the preload
library's constructor, or the program's first `pthread_create`). This is from glibc's
`elf/dl-iteratephdr.c` and `elf/dl-load.c` as known up to 2.43; the 2.43 source was not at
hand to read again when this entry was written, and a reviewer should check it.

musl is not called: there `static_tls` is 0 (BACKGROUND-03, I6).
