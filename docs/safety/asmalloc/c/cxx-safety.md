# Safety ledger: `crates/asmalloc/src/c/cxx.rs`

The C++ allocation operators, the new-handler, the forwarding to a program's replacement
operators, and the `export_cxx_abi!` macro. The lookups themselves are `loader.rs`'s
(`loader-safety.md`, LOADER-01 and LOADER-02; they were CXX-01 and CXX-02 here, including the
case of a failing nothrow `new`, whose new-handler lookup takes the loader's lock).

## CXX-03: the transmute to `GetNewHandler`, and the call through it

**Preconditions.** The non-null address `loader::first` returned is the entry of a function
with the signature `std::new_handler get_new_handler() noexcept`; calling it is safe.

**Proof.** `_ZSt15get_new_handlerv` is the Itanium C++ ABI name of `std::get_new_handler()`,
the same in libstdc++ and libc++, which define it with that signature (`<new>`: `new_handler
get_new_handler() noexcept;`). A symbol of that name defined by anything else would be a
program defining a reserved name of the standard library, which the C++ standard forbids. The
function takes no arguments, returns a function pointer in `rax` (null when no handler is
installed, which `Option<NewHandler>` represents with its null niche), is `noexcept` (so the
non-unwinding `extern "C"` ABI is right), and has no preconditions. Function pointers and
addresses have the same size and representation on x86-64 Linux, so the transmute of the
address is exact.

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

## CXX-05: the hand-off of a nothrow `new` to the C++ runtime's

**Block.** In `operator_new_nothrow` and `operator_new_aligned_nothrow`, the transmute of the
cached address to `fn(usize, *const u8) -> *mut c_void` (or with the alignment between) and
the call with `&NOTHROW`.

**Preconditions.** The address is the entry of the C++ runtime's `operator new(std::size_t,
const std::nothrow_t &) noexcept` (or the aligned one); the arguments are valid for it; it does
not unwind.

**Proof.** The address is `RTLD_NEXT`'s definition of `_ZnwmRKSt9nothrow_t` (or
`_ZnwmSt11align_val_tRKSt9nothrow_t`), found only when the basic `operator new` the program
binds to differs from the next one, and checked not to be ours (`runtime_nothrow`). Those
names are the Itanium manglings of the standard's replaceable nothrow forms; any definition
of them is a replacement the standard allows, with the standard's signature, and `noexcept`,
so the non-unwinding ABI is right. `size` and `align` are the caller's, passed on unchanged;
the alignment's contract (a power of two) is the caller's own under the same standard. The
`const std::nothrow_t &` is the address of a static byte, valid for the life of the program;
`std::nothrow_t` is an empty class, so the function has nothing to read through it. The
result is a block the program's `new` made, which the caller frees with the program's
`delete`: no block of ours is involved.

## CXX-06: `seam::free_small(class, object)` in `release`

**Precondition** (SEAM-01). `object` starts a slot of `class` that the heap handed out, owned
by the caller and given up.

**Proof.** `release` is reached only when the program has not replaced the basic `delete` of
the family (`DELETE`, `DELETE_ALIGNED`), so every `new` of the process that pairs with this
`delete` is ours or the C++ runtime's default, whose blocks come from `malloc` and
`aligned_alloc`, ours too under the C interface the macro requires (CXX-EXPORT-01). The
`delete`'s contract then says `ptr` is null or a block of the heap the caller owns and gives
up, and with `find` returning `Small(class)` the proof is SEAM-02's. A pointer `find` does not
know is not freed by us at all (CXX-07).

## CXX-07: the calls to the program's `operator delete` and `free`

**Block.** The transmute of a cached address to `fn(*mut c_void)` (or `fn(*mut c_void,
usize)` for the aligned form) and the call with `ptr` (and the alignment), in
`operator_delete_slow`, `operator_delete_aligned` and `release`.

**Preconditions.** The address is the entry of a function with that signature that does not
unwind; `ptr` meets its contract.

**Proof.** The addresses are the first definitions in the process of `_ZdlPv`,
`_ZdlPvSt11align_val_t` and `free`, found with `RTLD_DEFAULT` and kept only if they are not
in the object that holds the allocator (`theirs`, `loader::is_ours`). The first two are the
program's replacements of `operator delete(void*) noexcept` and `operator delete(void*,
std::align_val_t) noexcept`, which the standard lets a program define with exactly those
signatures; `free` is the program's `void free(void *)`. None unwinds. The call is what the
standard's default of the form being served does: every unaligned `delete` form's default
ends in `operator delete(void*)` with the same pointer, and every aligned one in `operator
delete(void*, std::align_val_t)` with the same pointer and alignment. So the program's
function gets exactly the arguments it would have got without us, and `ptr`'s contract is the
caller's, unchanged. `free` is called only when no `delete` is replaced and `find` does not
know the pointer: a pointer outside the heap, which by the `delete`'s contract a `new` of the
process returned; with no replaced `new` that is the C++ runtime's, built on the `malloc` the
program binds to, which is the program's own when its `free` is (`own_malloc.cpp`).

What this does not cover: a program that replaces `operator delete[]` without `operator
delete`, or replaces only some nothrow forms, gets the scalar family's answer; the standard's
defaults would send it to its array form. And until the generated `delete` routines read a
copy of the heap's location of their own that stays empty while the basic `delete` is
replaced, a replaced `new` whose blocks lie inside our heap (a header in front of a block from
our `malloc`) can have them taken by the routines' fast path: that case is the
`a_cxx_program_with_its_own_operator_new_runs` test, ignored until then.

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
C++ contract is that the pointer is null or was returned by a matching `new` and not yet freed,
which is the forwarded function's precondition. A sized form's size is not used; an aligned
form's alignment is used only to pass it to the program's aligned `delete` (CXX-07). So a
program that breaks the rule (a base pointer with no virtual destructor) still frees
correctly. None of the forwarding functions unwinds, so their non-unwinding ABI is right.

**The four throwing forms stay the C++ runtime's.** They are not exported, because they must
throw `std::bad_alloc` and a Rust frame here cannot let it pass (CXX-04). libstdc++'s
`operator new(size)` loops on `malloc` and the new-handler and throws; its aligned form does
the same with `aligned_alloc` (`libsupc++/new_op.cc`, `new_opa.cc`). Under the preload library,
and in a program that exports the C interface, those are asmalloc's, so every block the C++
runtime's `new` returns is a block of asmalloc's heap, and the `delete` here, which takes any
block of the heap, frees it correctly. The pairing is tested for every throwing form with every
matching `delete` form (`cxx_operators.cpp`). The macro refuses to build without the C
interface, which would pair libstdc++'s `new` (from glibc's `malloc`) with asmalloc's `delete`.

**A program that replaces operators.** A program, or a library loaded before ours, may define
the basic `new` and `delete` itself, or its own `malloc`. Then the forms it did not define are
still ours, and CXX-05 to CXX-07 make them behave as the standard's defaults: our nothrow `new`
hands off to the runtime's, which calls the program's `new`; our `delete` forms forward every
pointer to the program's basic `delete`; and with only `malloc` replaced, a `delete` of a block
from the runtime's `new` goes to the program's `free`.
