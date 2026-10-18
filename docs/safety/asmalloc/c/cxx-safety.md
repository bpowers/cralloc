# Safety ledger: `crates/asmalloc/src/c/cxx.rs`

The C++ allocation operators, the new-handler lookup, and the `export_cxx_abi!` macro. The
frees go through the seam: SEAM-02 (`seam-safety.md`) covers them.

## CXX-01: the declaration of `dlsym`

**Obligation.** The declaration matches the definition the linker binds it to, and the `link`
attribute names a library that defines it.

**Proof.** glibc declares `extern void *dlsym (void *__restrict __handle, const char
*__restrict __name) __THROW` (`dlfcn.h`), and since glibc 2.34 defines it in `libc.so.6`
(`dlsym@GLIBC_2.34`; checked with `nm -D` on the built library). The Rust declaration is
`extern "C" fn(*mut c_void, *const c_char) -> *mut c_void`: two pointer arguments and a
pointer result, the C calling convention. `#[link(name = "c")]` links that library. `__THROW`
says it does not throw, so a non-unwinding ABI is right.

## CXX-02: `dlsym(RTLD_DEFAULT, "_ZSt15get_new_handlerv")` in `new_handler`

**Preconditions.** The handle is valid for `dlsym`; the name is a NUL-terminated string that
lives through the call; the call is safe to make where `new_handler` runs.

**Proof.** `RTLD_DEFAULT` is `((void *) 0)` in glibc's `dlfcn.h`, the documented handle for a
search of the global scope. The name is a `&'static CStr`. `dlsym` takes the loader's lock,
which is recursive, so a call from a thread that holds it (a constructor running inside
`dlopen` that calls `new`) does not deadlock; glibc resets the lock in a forked child. It may
allocate (an error string when the symbol is missing), which reaches asmalloc again; the
allocator holds nothing across this call (the failed attempt has returned), and every one of
its paths may be entered from any point of another, so the nested call is an ordinary one.
`new_handler` runs only after an allocation failed, never inside the allocator.

## CXX-03: the transmute to `GetNewHandler`, and the call through it

**Preconditions.** The non-null address `dlsym` returned is the entry of a function with the
signature `std::new_handler get_new_handler() noexcept`; calling it is safe.

**Proof.** `_ZSt15get_new_handlerv` is the Itanium C++ ABI name of `std::get_new_handler()`,
the same in libstdc++ and libc++, which define it with that signature (`<new>`: `new_handler
get_new_handler() noexcept;`). A symbol of that name defined by anything else would be a
program defining a reserved name of the standard library, which the C++ standard forbids. The
function takes no arguments, returns a function pointer in `rax` (null when no handler is
installed, which `Option<NewHandler>` represents with its null niche), is `noexcept` (so the
non-unwinding `extern "C"` ABI is right), and has no preconditions. Function and data pointers
have the same size and representation on x86-64 Linux, so the transmute of the address is
exact.

## CXX-04: `handler()` in `retry`

**Preconditions.** `handler` is a function that may be called with no arguments; if it unwinds,
the unwinding is defined behaviour.

**Proof.** `handler` is the value `std::get_new_handler()` returned, non-null: the
`std::new_handler` the program installed with `std::set_new_handler`, a `void (*)()`. The C++
standard requires of a new-handler that it make memory available and return, throw
`std::bad_alloc` (or a type derived from it), or not return. The pointer type is `extern
"C-unwind"`, so an exception from the handler is allowed to reach this frame, where Rust
defines what happens: the allocator is built with `panic = "abort"`, so the call has a
landing pad that aborts the process (the unwinder's personality for the frame aborts in the
preload library, and calls `panic_cannot_unwind` with std). The process ends; no frame is left
half done. The handler may itself allocate; the allocator holds nothing across the call.

## CXX-EXPORT-01: the `export_cxx_abi!` forwarding calls and its `#[unsafe(no_mangle)]` items

**Obligations.** As C-EXPORT-01 (`mod-safety.md`): each forwarding `unsafe` call meets its
callee's contract, and each exported symbol is a sound definition of the C++ operator its name
mangles.

**Proof.** The names are the Itanium C++ ABI manglings of 16 of the 20 replaceable forms in
`<new>` on x86-64 (`size_t` is `m`, `std::align_val_t` is `St11align_val_t`, `const
std::nothrow_t &` is `RKSt9nothrow_t`): the four nothrow forms of `new` and the twelve of
`delete`, checked against libstdc++'s exports (`nm -D /usr/lib64/libstdc++.so.6`). The loader
binds each of them in a C++ program to the library, and the four throwing forms of `new` to
libstdc++ (`LD_DEBUG=bindings`, tested in `crates/asmalloc-preload/tests/preload.rs`). Each
argument has the C++ parameter's representation: `std::size_t` and the `enum class
std::align_val_t : std::size_t` are `usize`, a `const std::nothrow_t &` is a pointer, which is
ignored. Each `new` form's contract is C++'s: no preconditions on the size; the alignment of an
aligned form is a power of two (one that is not is refused, not served). The `delete` forms'
C++ contract is that the pointer is null or was returned by a matching `new` and not yet freed:
a block of the heap the caller owns and gives up, which is the forwarded function's
precondition. A sized form's size and an aligned form's alignment are, by C++'s rules, the
values given at allocation; the functions do not depend on that, so a program that breaks the
rule (a base pointer with no virtual destructor) still frees correctly. None of the
forwarding functions unwinds, so their non-unwinding ABI is right.

**The four throwing forms stay the C++ runtime's.** They are not exported, because they must
throw `std::bad_alloc` and a Rust frame here cannot let it pass (CXX-04). libstdc++'s
`operator new(size)` loops on `malloc` and the new-handler and throws; its aligned form does
the same with `aligned_alloc` (`libsupc++/new_op.cc`, `new_opa.cc`). Under the preload library,
and in a program that exports the C interface, those are asmalloc's, so every block the C++
runtime's `new` returns is a block of asmalloc's heap, and the `delete` here, which takes any
block of the heap, frees it correctly. The pairing is tested for every throwing form with every
matching `delete` form (`cxx_operators.cpp`). A program that exports these operators without
the C interface would pair libstdc++'s `new` (from glibc's `malloc`) with asmalloc's `delete`;
the `cxx-abi` feature and `export_cxx_abi!` are meant to be used with the C interface, and
their documentation says so.

So no block is allocated by one implementation and freed by another. A program that defines its own global
operators must not invoke the macro (or enable `cxx-abi`), which is why it is separate from
`export_c_abi!`.
