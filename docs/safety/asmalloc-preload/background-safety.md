# Safety ledger: `crates/asmalloc-preload/src/background.rs`

The preload library's background thread: an interposed `pthread_create` that creates the
allocator's thread, dormant, at the program's first thread, and a constructor that starts it
running before `main` when `ASMALLOC_BACKGROUND` asks. The thread itself is created by
`asmalloc::Asmalloc::start_dormant` (BACKGROUND-03, `docs/safety/asmalloc/heap/background-safety.md`).

## PRELOAD-03: the declarations of `dlsym` and `getpid`

**Obligation.** Each declaration matches the definition the linker binds it to, and the
`link` attribute names a library that defines them.

**Proof.** `void *dlsym(void *, const char *)`: `fn(*mut c_void, *const c_char) -> *mut
c_void`. `pid_t getpid(void)`: `pid_t` is `int` on Linux, `fn() -> c_int`. glibc 2.34 and
later define both in `libc.so.6` (`dlsym` moved there from `libdl`), and musl's `libc.so`
too. The library already needs the C library (ERRNO-01).

## PRELOAD-04: `#[used] #[unsafe(link_section = ".init_array")] static CONSTRUCTOR`

**Obligations.** An entry of `.init_array` is a pointer to a function the dynamic loader may
call with the C calling convention; the function is sound to run when the loader runs it.

**Proof.** The static is one `extern "C" fn()`, placed in `.init_array`, kept by `#[used]`,
relocated by the loader before any initializer runs. glibc calls it with three arguments and
musl with none; a function of none ignores them under System V. The loader runs it after the
C library is initialized. It reads the environment (BACKGROUND-02) and, if asked, looks up
the next `pthread_create` (PRELOAD-05) and starts the thread (PRELOAD-06). The heap needs no
initialization, so the allocations these make are ordinary. It does not unwind: the library
is built with `panic = "abort"`.

## PRELOAD-05: `dlsym(RTLD_NEXT, "pthread_create")` and the transmute of its result

**Preconditions.** The name is NUL-terminated. A non-null result is the address of a function
with `pthread_create`'s C signature, which `CreateThread` is.

**Proof.** `RTLD_NEXT` is `(void *)-1` in glibc and musl. The name is a `&'static CStr`.
`dlsym` returns the address of the definition of `pthread_create` in the objects after this
one in the lookup order, which is the C library's (the preload library comes first, so the
next definition is the one the program would have bound without it); null if there is none,
which the code checks before the transmute and answers with `EAGAIN`. A function pointer and
a data pointer are both 8 bytes on x86-64, and POSIX requires that `dlsym`'s result be
convertible to a function pointer. `dlsym` may allocate (glibc's error buffer), which re-enters
the library's allocator like any other call; it holds no allocator state. Racing first callers
may both look it up and store the same value.

## PRELOAD-06: `start_dormant(create)` and the forwarding `create(thread, attr, start, arg)`

**Preconditions.** `create` has `pthread_create`'s contract (BACKGROUND-03), and the program's
call is forwarded with its own arguments.

**Proof.** `create` is the next `pthread_create` (PRELOAD-05), the C library's. The interposer
is `pthread_create`'s own definition with its exact C signature (`#[unsafe(no_mangle)] extern
"C"`, the types of PRELOAD-03's sibling, BACKGROUND-01), so its caller's arguments carry the
caller's contract, and they are passed through unchanged; the result is returned unchanged.
The allocator's thread is created first, through the same `create`, from no slow path: the
program's call to `pthread_create` is a moment at which creating a thread is allowed, since
the program is creating one itself. A program that calls `pthread_create` from a signal
handler or under a lock that thread creation takes is outside POSIX already. `CREATED_IN`
keeps all but a process's first call from trying: a forked child's ID differs from its
parent's, so its first call tries for itself. It is a filter and nothing rests on it. A
process whose ID equals the one recorded (a descendant that was given the ID of an ancestor
that has exited, or the first process of a new PID namespace forked by a process that is
itself number 1) does not try, and gets no thread from this path; a call that does try while
a thread is running creates none, because the claim in `start_thread` is one compare-and-swap
of the thread's word, keyed by the process generation (`thread_word`). If the start fails the
program's thread is still created.

Reviewed: 3b5a3f5
