# The background thread's stack

2026-10-26. glibc 2.43, Linux 7.2, x86-64, rustc of the workspace's toolchain.

## Finding

What was measured and what rests on memory of glibc's source is marked: the 2.43 source was
not at hand, and the statements marked *from memory* are what a reviewer should check
against it.

The thread asked for a fixed 256 KiB stack. *Measured:* glibc puts a thread's static
thread-local data and a block of its own at the top of the stack it is asked for, and
refuses the request (`EINVAL`) only when the stack is about 2 KiB larger than those or less.
A program whose static TLS lands just under that limit got a thread with almost no stack,
which crashed in its first round of work. *From memory of glibc's source* (`nptl/allocatestack.c`,
`elf/dl-tls.c`): the test is `size < guard + tls_static_size_for_stack() +
MINIMAL_REST_STACK` rounded up to a page, `MINIMAL_REST_STACK` is 2048, and glibc's block is
the thread descriptor plus the static TLS surplus, which `glibc.rtld.optional_static_tls`
enlarges.

Both halves were reproduced (*measured*):

- A C program (`__thread char big[N]`, a thread with `pthread_attr_setstacksize(256 KiB)`
  that calls `printf`): N = 254,000 gave the thread 3,495 bytes below its locals and ran; N
  = 255,000 and 255,500 were accepted and crashed with SIGSEGV; N = 256,000 and above were
  refused with `EINVAL`.
- A Rust program with asmalloc as its allocator and a 252,000-byte `thread_local!` array
  (`PT_TLS` memsz 0x3d8c8, checked with `readelf -lW`): `Asmalloc::start_background()`
  succeeded with about 5 KiB of stack below the TLS, and the thread died with SIGSEGV on the
  first burst of frees.

musl is not affected (*read in source*, the musl checkout at 1b76ff07, not measured): its
`pthread_create` maps the requested size plus the TLS and its own block (`size = guard +
ROUND(stacksize + libc.tls_size + __pthread_tsd_size)`).

## How deep the thread goes

Method: `crates/asmalloc/tests/thread_stack.rs`. The thread's stack is fresh memory, zero
until written, so after the thread has run through a burst freed, a fragmented heap, growth
over the cap, parking and a wake, the lowest non-zero byte below the stack's top is the
deepest it has gone. The stack is found from the thread's stack pointer while it waits in a
system call, and bounded by its guard pages, which `/proc/self/mem` cannot read. All of the
table is *measured*, except the split of the top: "about 4.5 KiB" for glibc's block is the
distance from the top to the stack pointer of the parked thread less the few frames of its
wait, an estimate, not a reading of glibc's structures.

| Build | Below the top, all of it | Of which TLS and glibc's block | The thread's frames |
|---|---|---|---|
| Release, 104 B of TLS | 10.9 KiB | about 4.5 KiB | about 6.5 KiB |
| Debug, 104 B of TLS | 35.1 KiB | about 4.5 KiB | about 31 KiB |
| Release, 252,104 B of TLS | 264,656 B | 252,104 B and about 4.5 KiB | about 8 KiB |
| Debug, 252,104 B of TLS | 290,232 B | 252,104 B and about 4.5 KiB | about 34 KiB |

So with the old request the band of static TLS sizes that crashed was about 6 KiB wide in a
release build and about 30 KiB wide in a debug one, just under 256 KiB.

## What the thread asks for now

128 KiB for its frames (four times the debug figure), 64 KiB for glibc's block (about 4.5
KiB by default; more only if `glibc.rtld.optional_static_tls` is raised), and on glibc the
sum of the loaded modules' `PT_TLS` segments, found with `dl_iterate_phdr`. With the 252,000
bytes of TLS the stack is 440 KiB and the thread runs (*measured*). A stack is mapped lazily, so the part
never written costs address space only, and below 2 MiB no huge page can back it.
