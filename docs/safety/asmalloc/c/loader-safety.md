# Safety ledger: `crates/asmalloc/src/c/loader.rs`

The dynamic loader's symbol lookups that the C++ layer uses: `dlsym` for the new-handler and
for the program's replacement operators, and `dladdr` to tell whether a function is defined in
the object that holds the allocator.

## LOADER-01: the declarations of `dlsym` and `dladdr`

**Obligation.** Each declaration matches the definition the linker binds it to, and the `link`
attribute names a library that defines it.

**Proof.** glibc declares `extern void *dlsym (void *__restrict __handle, const char
*__restrict __name) __THROW` and `extern int dladdr (const void *__address, Dl_info *__info)
__THROW` (`dlfcn.h`), and since glibc 2.34 defines both in `libc.so.6` (`dlsym@GLIBC_2.34`,
`dladdr@GLIBC_2.34`). The Rust declarations are `extern "C" fn(*mut c_void, *const c_char) ->
*mut c_void` and `extern "C" fn(*const c_void, *mut Info) -> c_int`, the C calling convention
with pointer and `int` arguments of the same representation. `Info` is `#[repr(C)]` with the
four pointer-sized fields of `Dl_info` in order (`dli_fname`, `dli_fbase`, `dli_sname`,
`dli_saddr`). `#[link(name = "c")]` links that library. `__THROW` says neither throws, so a
non-unwinding ABI is right.

## LOADER-02: `dlsym(handle, name)` in `lookup`

**Preconditions.** The handle is valid for `dlsym`; the name is a NUL-terminated string that
lives through the call; the call is safe to make where `lookup` runs.

**Proof.** The handles are `RTLD_DEFAULT`, `((void *) 0)`, and `RTLD_NEXT`, `((void *) -1l)`,
in glibc's `dlfcn.h`: the global scope, and the objects after the caller's (the caller is
found from the return address, which is in this object). The name is a `&CStr`, alive for the
call. The result is only compared, or called through after a transmute that its own entry
justifies (CXX-03, CXX-05, CXX-07).

Where it runs: only on paths that hold nothing of the allocator's. `new_handler` runs after
an allocation failed; the cached lookups of `cxx.rs` run at the first `delete` or nothrow
`new` of a family, before anything is freed or allocated. `dlsym` takes the loader's lock,
which is recursive, so a call from a thread that holds it (a constructor running inside
`dlopen` that calls `new` or `delete`) does not deadlock, and glibc resets the lock in a
forked child. A thread in the middle of a `dlopen` holds the lock, and this call waits for it:
that is the one wait on another thread in the allocator, outside the "no locks" rule, and it
happens once per family per process (and on a failing `new`, CXX-02's case). `dlsym` may
allocate (an error string when the symbol is missing), which reaches asmalloc again; every one
of the allocator's paths may be entered from any point of another, so the nested call is an
ordinary one. A lookup from a signal handler that interrupted the loader on the same thread is
not covered: `dlsym` is not async-signal-safe, and a program that first deletes or fails to
allocate in such a handler gets glibc's behaviour for it.

## LOADER-03: `dladdr(address, &mut info)` in `object_of`

**Preconditions.** `info` is valid for a write of a `Dl_info`; the address may be anything;
the call is safe where it runs.

**Proof.** `info` is a local `Info`, exclusively borrowed. `dladdr` only compares the address
with the loader's list of mapped objects (`_dl_find_dso_for_object`) and never dereferences
it, so a pointer without provenance is fine. It reads `dli_fbase` only when the result is
non-zero, which is when glibc filled `info`. Where it runs and what it waits for: as
LOADER-02; it takes the same lock and does not allocate.
