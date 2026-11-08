# Safety ledger: `crates/cralloc/src/os/linux/rseq.rs`

The system calls that register a thread's own rseq area on musl, which registers none. Each
block calls `syscall()` (SYSCALL-01), so each entry's obligation is that the kernel's contract
for the call is met and that the call changes only what the caller may change. Compiled on musl
only; `front/area.rs`'s tests run every call on the real kernel.

References: rseq(2) and `kernel/rseq.c` (Linux 7.2), `include/uapi/linux/rseq.h`,
arch_prctl(2), rt_sigprocmask(2).

## RSEQ-01: `arch_prctl(ARCH_GET_FS)` in `thread_pointer`

On aarch64 `thread_pointer` has no block of its own: it reads the register (SYSCALL-03).

**Obligations.** The call writes 8 bytes through its second argument and nothing else.

**Proof.** `ARCH_GET_FS` stores the calling thread's FS base, an `unsigned long`, at the user
address given (`arch/x86/kernel/process_64.c`, `do_arch_prctl_64`), and changes no other state.
The address is `&mut base`, a live, exclusively borrowed local of 8 bytes. On failure (`EFAULT`
cannot happen for a local; nothing else is checked) the result is an error and `base` is not
read.

**Provenance.** The kernel writes the address as bytes; the pointer read back from them has no
provenance the abstract machine tracked, as the thread-local block it points into was never
allocated by Rust. The only use of it is `front::area`'s, which offsets it to the area and makes
the 4-byte atomic `cpu_id` access AREA-02 argues for. Neither Miri nor Kani runs this code.

## RSEQ-02: `rseq` registration (`register`)

**Obligations** (rseq(2)). `area` is aligned to 32 bytes and `len` is 32, or at least
`AT_RSEQ_FEATURE_SIZE` and aligned to `AT_RSEQ_ALIGN`; `signature` is what the abort handlers of
every sequence the thread will run carry; the area stays valid for the kernel to write until
the thread unregisters it or ends, and every descriptor `rseq_cs` may name stays mapped.

**Proof.** The function's `# Safety` passes the area's validity and alignment to the caller
(AREA-04). `len` is `RSEQ_AREA_LEN`, 32 (`ORIG_RSEQ_SIZE`, accepted by every kernel with rseq;
`kernel/rseq.c` checks `rseq_len == ORIG_RSEQ_SIZE` before the feature-size path). The
signature is `RSEQ_SIGNATURE_X86_64` or `_AARCH64`, the constant the Lean printer writes before every abort handler
(`VG.X86_64.rseqSignature`), emitted into `constants.rs` from the same definition, so the two
cannot differ. A wrong argument is rejected (`EINVAL`, `EPERM`), not undefined behaviour; a
second registration of the same area returns `EBUSY` and changes nothing.

## RSEQ-03: `rseq` unregistration (`unregister`)

**Obligations.** None beyond the kernel's own checks.

**Proof.** With `RSEQ_FLAG_UNREGISTER` the kernel compares the area, length and signature with
the thread's registration and fails with `EINVAL` or `EPERM` if they differ; otherwise it
writes the area's ID fields (`cpu_id` to -1) and forgets the registration. It reads and writes
only the registered area, which the thread registered (AREA-04), and nothing else.

## RSEQ-04: `rt_sigprocmask(SIG_SETMASK)` in `set_mask`

**Obligations.** The call reads 8 bytes from the new set and writes 8 to the old one; changing
the thread's signal mask is the caller's to do.

**Proof.** Both pointers are of live locals of 8 bytes, the kernel's `sigset_t` on x86-64 and aarch64,
whose size is passed. The kernel ignores `SIGKILL` and `SIGSTOP` in the set. The only caller is
the destructor that retires an exiting thread's area (AREA-05), which blocks signals for two
system calls and restores the mask it found, so code that runs after it sees the mask it had.

Reviewed: f6d9023
