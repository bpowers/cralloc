# Safety ledger: `crates/asmalloc/src/front/check.rs` and the test half of `front/sync.rs`

Both files are compiled only for this crate's own tests (`cfg(test)`); they never reach a build
of the allocator. They are recorded here because they live in the allocator crate.

## CHECK-01: `&*run` in `with_run`

**Proof.** `CONTEXT` holds a pointer only while `Run::thread` runs a scenario body on that
thread: it is set at the start and cleared before the function returns, also when the body
unwinds (the body runs inside `catch_unwind`). `Run::thread` is called through `&self` on a
`Run` that `explore` keeps on its stack until `thread::scope` has joined every thread that uses
it. So whenever the pointer is found, the `Run` is alive and only shared references to it exist.

## CHECK-02: `(*action)()` in `interrupt_point`, and the `transmute` in `interrupt_at`

**Proof.** The transmute only erases the lifetime of `&dyn Fn()`. `interrupt_at` stores the
pointer in the thread-local slot and creates a `Disarm` guard whose drop clears the slot, so the
pointer is removed before `interrupt_at` returns or unwinds, and the closure it points to is a
borrow that outlives the call. `interrupt_point` takes the pointer out of the slot before calling
it, so it is called at most once and only while `interrupt_at` is still on the stack.

## SYNC-01: `AtomicPtr::from_ptr` in the checked wrapper

**Proof.** The caller's obligations are those of `core::sync::atomic::AtomicPtr::from_ptr`, which
the wrapper calls with them unchanged. The wrapper is `#[repr(transparent)]` over the core type,
so a reference to the core value is a valid reference to the wrapper at the same address, with
the same lifetime.
