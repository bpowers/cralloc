//! Hand-written kernels. Register conventions for PAIR/BATCH kernels:
//!   rdi outer counter, r9 inner counter, rsi batch array, rbx state base,
//!   rdx aux (rseq offset from the thread pointer), r12 class index/offset,
//!   rax result of alloc and argument of free; rcx, r8, r10, r11 scratch.
use std::arch::global_asm;

global_asm!(
    r#"
.section .tbss,"awT",@nobits
.balign 64
.globl rf_tls
.type rf_tls,@object
.size rf_tls, 1024
rf_tls:
  .zero 1024

.section .bss
.balign 64
.globl rf_static
rf_static:
  .zero 256
.globl rf_abort_count
rf_abort_count:
  .quad 0
.balign 4096
.globl rf_slab
rf_slab:
  .zero 262144

.text
.globl rf_tls_addr
.type rf_tls_addr,@function
rf_tls_addr:
  mov rax, qword ptr fs:[0]
  lea rax, [rax + rf_tls@tpoff]
  ret
.size rf_tls_addr, .-rf_tls_addr

.globl rf_fs0
.type rf_fs0,@function
rf_fs0:
  mov rax, qword ptr fs:[0]
  ret
.size rf_fs0, .-rf_fs0

.balign 64
.globl rf_empty
.type rf_empty,@function
rf_empty:
  ret
.size rf_empty, .-rf_empty

.macro RSEQ_CS_DESC q
  .pushsection __rseq_cs, "aw"
  .balign 32
\q\()_cs:
  .long 0
  .long 0
  .quad \q\()_start
  .quad \q\()_end - \q\()_start
  .quad \q\()_abort
  .popsection
  .pushsection .text.unlikely, "ax"
  .long 0x53053053
\q\()_abort:
  inc qword ptr [rip + rf_abort_count]
  jmp \q\()_restart
  .popsection
.endm

/* ---- thread-local intrusive free list, head at a fixed TLS offset ---- */
.macro ALLOC_FL p
  mov rax, qword ptr fs:[rf_tls@tpoff]
  test rax, rax
  jz 9f
  mov rcx, qword ptr [rax]
  mov qword ptr fs:[rf_tls@tpoff], rcx
.endm
.macro FREE_FL p
  mov rcx, qword ptr fs:[rf_tls@tpoff]
  mov qword ptr [rax], rcx
  mov qword ptr fs:[rf_tls@tpoff], rax
.endm

/* ---- same, head inside a heap struct reached through a TLS pointer, class in a register ---- */
.macro ALLOC_FLH p
  mov r8, qword ptr fs:[rf_tls@tpoff + 64]
  mov rax, qword ptr [r8 + r12]
  test rax, rax
  jz 9f
  mov rcx, qword ptr [rax]
  mov qword ptr [r8 + r12], rcx
.endm
.macro FREE_FLH p
  mov r8, qword ptr fs:[rf_tls@tpoff + 64]
  mov rcx, qword ptr [r8 + r12]
  mov qword ptr [rax], rcx
  mov qword ptr [r8 + r12], rax
.endm

/* ---- thread-local array stack ("magazine"): one 64-bit top pointer, 512-byte aligned
        magazine, bounds encoded in the low bits of the pointer ---- */
.macro ALLOC_ARR p
  mov rcx, qword ptr fs:[rf_tls@tpoff + 8]
  sub rcx, 8
  test ecx, 0x1f8
  jz 9f
  mov rax, qword ptr [rcx]
  mov qword ptr fs:[rf_tls@tpoff + 8], rcx
.endm
.macro FREE_ARR p
  mov rcx, qword ptr fs:[rf_tls@tpoff + 8]
  test ecx, 0x1f8
  jz 9f
  mov qword ptr [rcx], rax
  add rcx, 8
  mov qword ptr fs:[rf_tls@tpoff + 8], rcx
.endm

/* ---- thread-local array stack with explicit bound loads ---- */
.macro ALLOC_ARRB p
  mov rcx, qword ptr fs:[rf_tls@tpoff + 8]
  cmp rcx, qword ptr fs:[rf_tls@tpoff + 16]
  je 9f
  mov rax, qword ptr [rcx - 8]
  sub rcx, 8
  mov qword ptr fs:[rf_tls@tpoff + 8], rcx
.endm
.macro FREE_ARRB p
  mov rcx, qword ptr fs:[rf_tls@tpoff + 8]
  cmp rcx, qword ptr fs:[rf_tls@tpoff + 24]
  je 9f
  mov qword ptr [rcx], rax
  add rcx, 8
  mov qword ptr fs:[rf_tls@tpoff + 8], rcx
.endm

/* ---- thread-local 64-bit bitmap (one word = one 64-object run) ---- */
.macro ALLOC_BM p
  mov rcx, qword ptr fs:[rf_tls@tpoff + 48]
  tzcnt rax, rcx
  jc 9f
  blsr rcx, rcx
  mov qword ptr fs:[rf_tls@tpoff + 48], rcx
  shl rax, 6
  add rax, qword ptr fs:[rf_tls@tpoff + 56]
.endm
.macro FREE_BM p
  mov r8, rax
  sub r8, qword ptr fs:[rf_tls@tpoff + 56]
  shr r8, 6
  mov rcx, qword ptr fs:[rf_tls@tpoff + 48]
  bts rcx, r8
  jc 9f
  mov qword ptr fs:[rf_tls@tpoff + 48], rcx
.endm

/* ---- tcmalloc's per-CPU slab Pop/Push, instruction for instruction
        (percpu_tcmalloc.h:877 and :743), r12 = size class index ---- */
.macro ALLOC_TCMX p, pf
\p\()_pop_restart:
  lea r10, [rip + \p\()_pop_cs]
  mov qword ptr fs:[rdx + 8], r10
\p\()_pop_start:
  mov r8, qword ptr fs:[rf_tls@tpoff + 72]
  btr r8, 63
  jnc 9f
  movzx ecx, word ptr [r8 + r12*4]
  mov rax, qword ptr [r8 + rcx*8 - 8]
  test al, 1
  jnz 9f
  mov r11, qword ptr [r8 + rcx*8 - 16]
  sub dword ptr [r8 + r12*4], 0xffff0001
\p\()_pop_end:
  .if \pf
  prefetchw byte ptr [r8 + rcx*8 - 24]
  prefetcht0 byte ptr [r11]
  .endif
  RSEQ_CS_DESC \p\()_pop
.endm
.macro FREE_TCMX p, pf
\p\()_push_restart:
  lea r10, [rip + \p\()_push_cs]
  mov qword ptr fs:[rdx + 8], r10
\p\()_push_start:
  mov r8, qword ptr fs:[rf_tls@tpoff + 72]
  btr r8, 63
  jnc 9f
  mov r11d, dword ptr [r8 + r12*4]
  movzx ecx, r11w
  add r11d, 0xffff0001
  jnc 9f
  mov qword ptr [r8 + rcx*8], rax
  mov dword ptr [r8 + r12*4], r11d
\p\()_push_end:
  .if \pf
  prefetchw byte ptr [r8 + rcx*8 + 8]
  .endif
  RSEQ_CS_DESC \p\()_push
.endm
.macro ALLOC_TCM p
  ALLOC_TCMX \p, 1
.endm
.macro FREE_TCM p
  FREE_TCMX \p, 1
.endm
.macro ALLOC_TCMNP p
  ALLOC_TCMX \p, 0
.endm
.macro FREE_TCMNP p
  FREE_TCMX \p, 0
.endm

/* ---- per-CPU magazine under rseq: region = base + (mm_cid << 12), one 64-bit top pointer
        per class, bounds in the pointer's low bits, single committing store ---- */
.macro ALLOC_RQA p
\p\()_pop_restart:
  lea r10, [rip + \p\()_pop_cs]
  mov qword ptr fs:[rdx + 8], r10
\p\()_pop_start:
  mov r8d, dword ptr fs:[rdx + 24]
  shl r8, 12
  add r8, rbx
  mov rcx, qword ptr [r8 + r12]
  sub rcx, 8
  test ecx, 0x1f8
  jz 9f
  mov rax, qword ptr [rcx]
  mov qword ptr [r8 + r12], rcx
\p\()_pop_end:
  RSEQ_CS_DESC \p\()_pop
.endm
.macro FREE_RQA p
\p\()_push_restart:
  lea r10, [rip + \p\()_push_cs]
  mov qword ptr fs:[rdx + 8], r10
\p\()_push_start:
  mov r8d, dword ptr fs:[rdx + 24]
  shl r8, 12
  add r8, rbx
  mov rcx, qword ptr [r8 + r12]
  test ecx, 0x1f8
  jz 9f
  mov qword ptr [rcx], rax
  add rcx, 8
  mov qword ptr [r8 + r12], rcx
\p\()_push_end:
  RSEQ_CS_DESC \p\()_push
.endm

/* ---- the same, but the rseq offset and the per-CPU base are reloaded from memory on every
        operation, as an out-of-line malloc in a shared object would have to ---- */
.macro ALLOC_RQG p
\p\()_pop_restart:
  mov rdx, qword ptr [rip + rf_static + 16]
  lea r10, [rip + \p\()_pop_cs]
  mov qword ptr fs:[rdx + 8], r10
\p\()_pop_start:
  mov r8d, dword ptr fs:[rdx + 24]
  shl r8, 12
  add r8, qword ptr [rip + rf_static + 24]
  mov rcx, qword ptr [r8 + r12]
  sub rcx, 8
  test ecx, 0x1f8
  jz 9f
  mov rax, qword ptr [rcx]
  mov qword ptr [r8 + r12], rcx
\p\()_pop_end:
  RSEQ_CS_DESC \p\()_pop
.endm
.macro FREE_RQG p
\p\()_push_restart:
  mov rdx, qword ptr [rip + rf_static + 16]
  lea r10, [rip + \p\()_push_cs]
  mov qword ptr fs:[rdx + 8], r10
\p\()_push_start:
  mov r8d, dword ptr fs:[rdx + 24]
  shl r8, 12
  add r8, qword ptr [rip + rf_static + 24]
  mov rcx, qword ptr [r8 + r12]
  test ecx, 0x1f8
  jz 9f
  mov qword ptr [rcx], rax
  add rcx, 8
  mov qword ptr [r8 + r12], rcx
\p\()_push_end:
  RSEQ_CS_DESC \p\()_push
.endm

/* ---- per-CPU intrusive free list under rseq ---- */
.macro ALLOC_RQF p
\p\()_pop_restart:
  lea r10, [rip + \p\()_pop_cs]
  mov qword ptr fs:[rdx + 8], r10
\p\()_pop_start:
  mov r8d, dword ptr fs:[rdx + 24]
  shl r8, 12
  add r8, rbx
  mov rax, qword ptr [r8 + r12 + 2048]
  test rax, rax
  jz 9f
  mov rcx, qword ptr [rax]
  mov qword ptr [r8 + r12 + 2048], rcx
\p\()_pop_end:
  RSEQ_CS_DESC \p\()_pop
.endm
.macro FREE_RQF p
\p\()_push_restart:
  lea r10, [rip + \p\()_push_cs]
  mov qword ptr fs:[rdx + 8], r10
\p\()_push_start:
  mov r8d, dword ptr fs:[rdx + 24]
  shl r8, 12
  add r8, rbx
  mov rcx, qword ptr [r8 + r12 + 2048]
  mov qword ptr [rax], rcx
  mov qword ptr [r8 + r12 + 2048], rax
\p\()_push_end:
  RSEQ_CS_DESC \p\()_push
.endm

/* ---- mimalloc-shaped traffic: theap -> pages_free_direct[class] -> page->free on alloc;
        two-level page map -> page, owner check, push on page->local_free on free.
        The "generic" path only swaps local_free into free. ---- */
.macro ALLOC_MI p
  mov r8, qword ptr fs:[rf_tls@tpoff + 64]
  mov r8, qword ptr [r8 + r12]
  mov rax, qword ptr [r8]
  test rax, rax
  jz \p\()_generic
\p\()_pop:
  mov rcx, qword ptr [rax]
  mov qword ptr [r8], rcx
  add dword ptr [r8 + 16], 1
  .pushsection .text.unlikely, "ax"
\p\()_generic:
  mov rax, qword ptr [r8 + 8]
  mov qword ptr [r8 + 8], 0
  test rax, rax
  jnz \p\()_pop
  ud2
  .popsection
.endm
.macro FREE_MI p
  mov rcx, rax
  shr rcx, 29
  mov r10, qword ptr [rip + rf_static + 8]
  mov r10, qword ptr [r10 + rcx*8]
  mov rcx, rax
  shr rcx, 16
  and ecx, 0x1fff
  mov r8, qword ptr [r10 + rcx*8]
  mov r10, qword ptr fs:[0]
  cmp r10, qword ptr [r8 + 24]
  jne 9f
  mov rcx, qword ptr [r8 + 8]
  mov qword ptr [rax], rcx
  mov qword ptr [r8 + 8], rax
  sub dword ptr [r8 + 16], 1
  jz 9f
.endm

/* ---- thread-local variants that first materialise the thread pointer in a register ---- */
.macro ALLOC_FL2 p
  mov r8, qword ptr fs:[0]
  mov rax, qword ptr [r8 + rf_tls@tpoff]
  test rax, rax
  jz 9f
  mov rcx, qword ptr [rax]
  mov qword ptr [r8 + rf_tls@tpoff], rcx
.endm
.macro FREE_FL2 p
  mov r8, qword ptr fs:[0]
  mov rcx, qword ptr [r8 + rf_tls@tpoff]
  mov qword ptr [rax], rcx
  mov qword ptr [r8 + rf_tls@tpoff], rax
.endm
.macro ALLOC_ARR2 p
  mov r8, qword ptr fs:[0]
  mov rcx, qword ptr [r8 + rf_tls@tpoff + 8]
  sub rcx, 8
  test ecx, 0x1f8
  jz 9f
  mov rax, qword ptr [rcx]
  mov qword ptr [r8 + rf_tls@tpoff + 8], rcx
.endm
.macro FREE_ARR2 p
  mov r8, qword ptr fs:[0]
  mov rcx, qword ptr [r8 + rf_tls@tpoff + 8]
  test ecx, 0x1f8
  jz 9f
  mov qword ptr [rcx], rax
  add rcx, 8
  mov qword ptr [r8 + rf_tls@tpoff + 8], rcx
.endm
.macro ALLOC_BM2 p
  mov r8, qword ptr fs:[0]
  mov rcx, qword ptr [r8 + rf_tls@tpoff + 48]
  tzcnt rax, rcx
  jc 9f
  blsr rcx, rcx
  mov qword ptr [r8 + rf_tls@tpoff + 48], rcx
  shl rax, 6
  add rax, qword ptr [r8 + rf_tls@tpoff + 56]
.endm
.macro FREE_BM2 p
  mov r8, qword ptr fs:[0]
  mov r10, rax
  sub r10, qword ptr [r8 + rf_tls@tpoff + 56]
  shr r10, 6
  mov rcx, qword ptr [r8 + rf_tls@tpoff + 48]
  bts rcx, r10
  jc 9f
  mov qword ptr [r8 + rf_tls@tpoff + 48], rcx
.endm

/* ---- dissections of the per-CPU magazine: no rseq arm; no mm_cid indexing ---- */
.macro ALLOC_RQN p
  mov r8d, dword ptr fs:[rdx + 24]
  shl r8, 12
  add r8, rbx
  mov rcx, qword ptr [r8 + r12]
  sub rcx, 8
  test ecx, 0x1f8
  jz 9f
  mov rax, qword ptr [rcx]
  mov qword ptr [r8 + r12], rcx
.endm
.macro FREE_RQN p
  mov r8d, dword ptr fs:[rdx + 24]
  shl r8, 12
  add r8, rbx
  mov rcx, qword ptr [r8 + r12]
  test ecx, 0x1f8
  jz 9f
  mov qword ptr [rcx], rax
  add rcx, 8
  mov qword ptr [r8 + r12], rcx
.endm
.macro ALLOC_RQH p
\p\()_pop_restart:
  lea r10, [rip + \p\()_pop_cs]
  mov qword ptr fs:[rdx + 8], r10
\p\()_pop_start:
  mov rcx, qword ptr [rbx + r12]
  sub rcx, 8
  test ecx, 0x1f8
  jz 9f
  mov rax, qword ptr [rcx]
  mov qword ptr [rbx + r12], rcx
\p\()_pop_end:
  RSEQ_CS_DESC \p\()_pop
.endm
.macro FREE_RQH p
\p\()_push_restart:
  lea r10, [rip + \p\()_push_cs]
  mov qword ptr fs:[rdx + 8], r10
\p\()_push_start:
  mov rcx, qword ptr [rbx + r12]
  test ecx, 0x1f8
  jz 9f
  mov qword ptr [rcx], rax
  add rcx, 8
  mov qword ptr [rbx + r12], rcx
\p\()_push_end:
  RSEQ_CS_DESC \p\()_push
.endm
/* hoisted everything: the bare magazine through a base register */
.macro ALLOC_RQ0 p
  mov rcx, qword ptr [rbx + r12]
  sub rcx, 8
  test ecx, 0x1f8
  jz 9f
  mov rax, qword ptr [rcx]
  mov qword ptr [rbx + r12], rcx
.endm
.macro FREE_RQ0 p
  mov rcx, qword ptr [rbx + r12]
  test ecx, 0x1f8
  jz 9f
  mov qword ptr [rcx], rax
  add rcx, 8
  mov qword ptr [rbx + r12], rcx
.endm
/* magazine addressed by a 32-bit index instead of a pointer: slot address does not depend on
   the popped top pointer's arithmetic, commit is the index */
.macro ALLOC_RQI p
\p\()_pop_restart:
  lea r10, [rip + \p\()_pop_cs]
  mov qword ptr fs:[rdx + 8], r10
\p\()_pop_start:
  mov r8d, dword ptr fs:[rdx + 24]
  shl r8, 12
  add r8, rbx
  mov ecx, dword ptr [r8 + r12 + 1024]
  sub ecx, 1
  js 9f
  mov rax, qword ptr [r8 + rcx*8 + 1088]
  mov dword ptr [r8 + r12 + 1024], ecx
\p\()_pop_end:
  RSEQ_CS_DESC \p\()_pop
.endm
.macro FREE_RQI p
\p\()_push_restart:
  lea r10, [rip + \p\()_push_cs]
  mov qword ptr fs:[rdx + 8], r10
\p\()_push_start:
  mov r8d, dword ptr fs:[rdx + 24]
  shl r8, 12
  add r8, rbx
  mov ecx, dword ptr [r8 + r12 + 1024]
  cmp ecx, 60
  jae 9f
  mov qword ptr [r8 + rcx*8 + 1088], rax
  add ecx, 1
  mov dword ptr [r8 + r12 + 1024], ecx
\p\()_push_end:
  RSEQ_CS_DESC \p\()_push
.endm

.macro ALLOC_E1 p
  lea r10, [rip + rf_static]
  
  mov rcx, qword ptr [rbx + r12]
  sub rcx, 8
  test ecx, 0x1f8
  jz 9f
  mov rax, qword ptr [rcx]
  mov qword ptr [rbx + r12], rcx
.endm
.macro FREE_E1 p
  lea r10, [rip + rf_static]
  
  mov rcx, qword ptr [rbx + r12]
  test ecx, 0x1f8
  jz 9f
  mov qword ptr [rcx], rax
  add rcx, 8
  mov qword ptr [rbx + r12], rcx
.endm

.macro ALLOC_E2 p
  mov qword ptr [rbx + 512], rdi
  
  mov rcx, qword ptr [rbx + r12]
  sub rcx, 8
  test ecx, 0x1f8
  jz 9f
  mov rax, qword ptr [rcx]
  mov qword ptr [rbx + r12], rcx
.endm
.macro FREE_E2 p
  mov qword ptr [rbx + 512], rdi
  
  mov rcx, qword ptr [rbx + r12]
  test ecx, 0x1f8
  jz 9f
  mov qword ptr [rcx], rax
  add rcx, 8
  mov qword ptr [rbx + r12], rcx
.endm

.macro ALLOC_E3 p
  mov qword ptr fs:[rdx + 8], 0
  
  mov rcx, qword ptr [rbx + r12]
  sub rcx, 8
  test ecx, 0x1f8
  jz 9f
  mov rax, qword ptr [rcx]
  mov qword ptr [rbx + r12], rcx
.endm
.macro FREE_E3 p
  mov qword ptr fs:[rdx + 8], 0
  
  mov rcx, qword ptr [rbx + r12]
  test ecx, 0x1f8
  jz 9f
  mov qword ptr [rcx], rax
  add rcx, 8
  mov qword ptr [rbx + r12], rcx
.endm

.macro ALLOC_E4 p
  mov r10d, dword ptr fs:[rdx + 24]
  
  mov rcx, qword ptr [rbx + r12]
  sub rcx, 8
  test ecx, 0x1f8
  jz 9f
  mov rax, qword ptr [rcx]
  mov qword ptr [rbx + r12], rcx
.endm
.macro FREE_E4 p
  mov r10d, dword ptr fs:[rdx + 24]
  
  mov rcx, qword ptr [rbx + r12]
  test ecx, 0x1f8
  jz 9f
  mov qword ptr [rcx], rax
  add rcx, 8
  mov qword ptr [rbx + r12], rcx
.endm

.macro ALLOC_E5 p
  
  mov r8, rbx
  mov rcx, qword ptr [r8 + r12]
  sub rcx, 8
  test ecx, 0x1f8
  jz 9f
  mov rax, qword ptr [rcx]
  mov qword ptr [r8 + r12], rcx
.endm
.macro FREE_E5 p
  
  mov r8, rbx
  mov rcx, qword ptr [r8 + r12]
  test ecx, 0x1f8
  jz 9f
  mov qword ptr [rcx], rax
  add rcx, 8
  mov qword ptr [r8 + r12], rcx
.endm

.macro ALLOC_E7 p
  nop
  nop
  nop
  
  mov rcx, qword ptr [rbx + r12]
  sub rcx, 8
  test ecx, 0x1f8
  jz 9f
  mov rax, qword ptr [rcx]
  mov qword ptr [rbx + r12], rcx
.endm
.macro FREE_E7 p
  nop
  nop
  nop
  
  mov rcx, qword ptr [rbx + r12]
  test ecx, 0x1f8
  jz 9f
  mov qword ptr [rcx], rax
  add rcx, 8
  mov qword ptr [rbx + r12], rcx
.endm

.macro ALLOC_E8 p
  
  xor r11d, r11d
  lea r8, [rbx + r11]
  mov rcx, qword ptr [r8 + r12]
  sub rcx, 8
  test ecx, 0x1f8
  jz 9f
  mov rax, qword ptr [rcx]
  mov qword ptr [r8 + r12], rcx
.endm
.macro FREE_E8 p
  
  xor r11d, r11d
  lea r8, [rbx + r11]
  mov rcx, qword ptr [r8 + r12]
  test ecx, 0x1f8
  jz 9f
  mov qword ptr [rcx], rax
  add rcx, 8
  mov qword ptr [r8 + r12], rcx
.endm

.macro ALLOC_E9 p
  
  mov r8, qword ptr [rip + rf_static + 24]
  mov rcx, qword ptr [r8 + r12]
  sub rcx, 8
  test ecx, 0x1f8
  jz 9f
  mov rax, qword ptr [rcx]
  mov qword ptr [r8 + r12], rcx
.endm
.macro FREE_E9 p
  
  mov r8, qword ptr [rip + rf_static + 24]
  mov rcx, qword ptr [r8 + r12]
  test ecx, 0x1f8
  jz 9f
  mov qword ptr [rcx], rax
  add rcx, 8
  mov qword ptr [r8 + r12], rcx
.endm

.macro ALLOC_E10 p
  
  mov r8d, dword ptr fs:[rdx + 24]
  add r8, rbx
  mov rcx, qword ptr [r8 + r12]
  sub rcx, 8
  test ecx, 0x1f8
  jz 9f
  mov rax, qword ptr [rcx]
  mov qword ptr [r8 + r12], rcx
.endm
.macro FREE_E10 p
  
  mov r8d, dword ptr fs:[rdx + 24]
  add r8, rbx
  mov rcx, qword ptr [r8 + r12]
  test ecx, 0x1f8
  jz 9f
  mov qword ptr [rcx], rax
  add rcx, 8
  mov qword ptr [r8 + r12], rcx
.endm

.macro ALLOC_E11 p
  
  mov r8, qword ptr [rip + rf_static + 24]
  add r8, 0
  mov rcx, qword ptr [r8 + r12]
  sub rcx, 8
  test ecx, 0x1f8
  jz 9f
  mov rax, qword ptr [rcx]
  mov qword ptr [r8 + r12], rcx
.endm
.macro FREE_E11 p
  
  mov r8, qword ptr [rip + rf_static + 24]
  add r8, 0
  mov rcx, qword ptr [r8 + r12]
  test ecx, 0x1f8
  jz 9f
  mov qword ptr [rcx], rax
  add rcx, 8
  mov qword ptr [r8 + r12], rcx
.endm

.macro ALLOC_E12 p
  lea r10, [rip + rf_static]
  mov qword ptr [rbx + 512], r10
  
  mov rcx, qword ptr [rbx + r12]
  sub rcx, 8
  test ecx, 0x1f8
  jz 9f
  mov rax, qword ptr [rcx]
  mov qword ptr [rbx + r12], rcx
.endm
.macro FREE_E12 p
  lea r10, [rip + rf_static]
  mov qword ptr [rbx + 512], r10
  
  mov rcx, qword ptr [rbx + r12]
  test ecx, 0x1f8
  jz 9f
  mov qword ptr [rcx], rax
  add rcx, 8
  mov qword ptr [rbx + r12], rcx
.endm

/* ---- the planned shape: the slab is a static (its address is a lea), the rseq offset is
        loaded from memory as glibc's __rseq_offset would be, the class offset is in r12 ---- */
.macro ALLOC_RQD p
\p\()_pop_restart:
  mov rdx, qword ptr [rip + rf_static + 16]
  lea r10, [rip + \p\()_pop_cs]
  mov qword ptr fs:[rdx + 8], r10
\p\()_pop_start:
  mov r8d, dword ptr fs:[rdx + 24]
  shl r8, 12
  lea r11, [rip + rf_slab]
  add r8, r11
  mov rcx, qword ptr [r8 + r12]
  sub rcx, 8
  test ecx, 0x1f8
  jz 9f
  mov rax, qword ptr [rcx]
  mov qword ptr [r8 + r12], rcx
\p\()_pop_end:
  RSEQ_CS_DESC \p\()_pop
.endm
.macro FREE_RQD p
\p\()_push_restart:
  mov rdx, qword ptr [rip + rf_static + 16]
  lea r10, [rip + \p\()_push_cs]
  mov qword ptr fs:[rdx + 8], r10
\p\()_push_start:
  mov r8d, dword ptr fs:[rdx + 24]
  shl r8, 12
  lea r11, [rip + rf_slab]
  add r8, r11
  mov rcx, qword ptr [r8 + r12]
  test ecx, 0x1f8
  jz 9f
  mov qword ptr [rcx], rax
  add rcx, 8
  mov qword ptr [r8 + r12], rcx
\p\()_push_end:
  RSEQ_CS_DESC \p\()_push
.endm
/* the same with the class a compile-time constant, as at an inlined call site */
.macro ALLOC_RQC p
\p\()_pop_restart:
  mov rdx, qword ptr [rip + rf_static + 16]
  lea r10, [rip + \p\()_pop_cs]
  mov qword ptr fs:[rdx + 8], r10
\p\()_pop_start:
  mov r8d, dword ptr fs:[rdx + 24]
  shl r8, 12
  lea r11, [rip + rf_slab]
  mov rcx, qword ptr [r11 + r8 + 64]
  sub rcx, 8
  test ecx, 0x1f8
  jz 9f
  mov rax, qword ptr [rcx]
  mov qword ptr [r11 + r8 + 64], rcx
\p\()_pop_end:
  RSEQ_CS_DESC \p\()_pop
.endm
.macro FREE_RQC p
\p\()_push_restart:
  mov rdx, qword ptr [rip + rf_static + 16]
  lea r10, [rip + \p\()_push_cs]
  mov qword ptr fs:[rdx + 8], r10
\p\()_push_start:
  mov r8d, dword ptr fs:[rdx + 24]
  shl r8, 12
  lea r11, [rip + rf_slab]
  mov rcx, qword ptr [r11 + r8 + 64]
  test ecx, 0x1f8
  jz 9f
  mov qword ptr [rcx], rax
  add rcx, 8
  mov qword ptr [r11 + r8 + 64], rcx
\p\()_push_end:
  RSEQ_CS_DESC \p\()_push
.endm

.macro PROLOGUE
  push rbx
  push r12
  mov rbx, rsi
  mov rsi, rcx
  mov r12, r8
.endm
.macro EPILOGUE
  pop r12
  pop rbx
  ret
9:
  ud2
.endm

/* alloc immediately followed by free of the same object */
.macro PAIR name, allocm, freem
  .text
  .balign 64
  .globl \name
  .type \name,@function
\name:
  PROLOGUE
  .balign 64
2:
  \allocm \name\()_a
  \freem \name\()_f
  dec rdi
  jnz 2b
  EPILOGUE
  .size \name, .-\name
.endm

/* the same through out-of-line functions, one call each */
.macro PAIRC name, allocm, freem
  .text
  .balign 64
\name\()_fa:
  \allocm \name\()_a
  ret
9:
  ud2
  .balign 64
\name\()_ff:
  \freem \name\()_f
  ret
9:
  ud2
  .balign 64
  .globl \name
  .type \name,@function
\name:
  PROLOGUE
  .balign 64
2:
  call \name\()_fa
  call \name\()_ff
  dec rdi
  jnz 2b
  EPILOGUE
  .size \name, .-\name
.endm

/* 32 allocations, then 32 frees in allocation order */
.macro BATCH name, allocm, freem
  .text
  .balign 64
  .globl \name
  .type \name,@function
\name:
  PROLOGUE
  .balign 64
2:
  xor r9d, r9d
  .balign 32
3:
  \allocm \name\()_a
  mov qword ptr [rsi + r9*8], rax
  inc r9
  cmp r9, 32
  jne 3b
  xor r9d, r9d
  .balign 32
4:
  mov rax, qword ptr [rsi + r9*8]
  \freem \name\()_f
  inc r9
  cmp r9, 32
  jne 4b
  dec rdi
  jnz 2b
  EPILOGUE
  .size \name, .-\name
.endm

PAIR  p_fl,     ALLOC_FL,    FREE_FL
PAIR  p_flh,    ALLOC_FLH,   FREE_FLH
PAIR  p_arr,    ALLOC_ARR,   FREE_ARR
PAIR  p_arrb,   ALLOC_ARRB,  FREE_ARRB
PAIR  p_bm,     ALLOC_BM,    FREE_BM
PAIR  p_tcm,    ALLOC_TCM,   FREE_TCM
PAIR  p_tcmnp,  ALLOC_TCMNP, FREE_TCMNP
PAIR  p_rqa,    ALLOC_RQA,   FREE_RQA
PAIR  p_rqg,    ALLOC_RQG,   FREE_RQG
PAIR  p_rqf,    ALLOC_RQF,   FREE_RQF
PAIR  p_mi,     ALLOC_MI,    FREE_MI
PAIR  p_fl2,    ALLOC_FL2,   FREE_FL2
PAIR  p_arr2,   ALLOC_ARR2,  FREE_ARR2
PAIR  p_bm2,    ALLOC_BM2,   FREE_BM2
PAIR  p_rqn,    ALLOC_RQN,   FREE_RQN
PAIR  p_rqh,    ALLOC_RQH,   FREE_RQH
PAIR  p_rq0,    ALLOC_RQ0,   FREE_RQ0
PAIR  p_rqi,    ALLOC_RQI,   FREE_RQI
PAIR  p_e1, ALLOC_E1, FREE_E1
PAIR  p_e2, ALLOC_E2, FREE_E2
PAIR  p_e3, ALLOC_E3, FREE_E3
PAIR  p_e4, ALLOC_E4, FREE_E4
PAIR  p_e5, ALLOC_E5, FREE_E5
PAIR  p_e7, ALLOC_E7, FREE_E7
PAIR  p_e8, ALLOC_E8, FREE_E8
PAIR  p_e9, ALLOC_E9, FREE_E9
PAIR  p_e10, ALLOC_E10, FREE_E10
PAIR  p_e11, ALLOC_E11, FREE_E11
PAIR  p_e12, ALLOC_E12, FREE_E12
PAIR  p_rqd,    ALLOC_RQD,   FREE_RQD
PAIR  p_rqc,    ALLOC_RQC,   FREE_RQC
PAIRC c_rqd,    ALLOC_RQD,   FREE_RQD
PAIRC c_rqc,    ALLOC_RQC,   FREE_RQC
BATCH b_rqd,    ALLOC_RQD,   FREE_RQD
BATCH b_rqc,    ALLOC_RQC,   FREE_RQC
PAIRC c_fl2,    ALLOC_FL2,   FREE_FL2
PAIRC c_arr2,   ALLOC_ARR2,  FREE_ARR2
PAIRC c_rqf,    ALLOC_RQF,   FREE_RQF
BATCH b_fl2,    ALLOC_FL2,   FREE_FL2
BATCH b_arr2,   ALLOC_ARR2,  FREE_ARR2
BATCH b_bm2,    ALLOC_BM2,   FREE_BM2
BATCH b_rq0,    ALLOC_RQ0,   FREE_RQ0
BATCH b_rqi,    ALLOC_RQI,   FREE_RQI

PAIRC c_fl,     ALLOC_FL,    FREE_FL
PAIRC c_arr,    ALLOC_ARR,   FREE_ARR
PAIRC c_tcm,    ALLOC_TCM,   FREE_TCM
PAIRC c_rqa,    ALLOC_RQA,   FREE_RQA
PAIRC c_rqg,    ALLOC_RQG,   FREE_RQG
PAIRC c_mi,     ALLOC_MI,    FREE_MI

BATCH b_fl,     ALLOC_FL,    FREE_FL
BATCH b_flh,    ALLOC_FLH,   FREE_FLH
BATCH b_arr,    ALLOC_ARR,   FREE_ARR
BATCH b_bm,     ALLOC_BM,    FREE_BM
BATCH b_tcm,    ALLOC_TCM,   FREE_TCM
BATCH b_tcmnp,  ALLOC_TCMNP, FREE_TCMNP
BATCH b_rqa,    ALLOC_RQA,   FREE_RQA
BATCH b_rqg,    ALLOC_RQG,   FREE_RQG
BATCH b_rqf,    ALLOC_RQF,   FREE_RQF
BATCH b_mi,     ALLOC_MI,    FREE_MI
"#
);

macro_rules! simple {
    ($name:ident, $setup:expr, $body:expr, $fini:expr) => {
        global_asm!(concat!(
            ".text\n.balign 64\n.globl ",
            stringify!($name),
            "\n.type ",
            stringify!($name),
            ",@function\n",
            stringify!($name),
            ":\n",
            $setup,
            "\n.balign 64\n2:\n",
            $body,
            "\ndec rdi\njnz 2b\n",
            $fini,
            "\nret\n9:\nud2\n.size ",
            stringify!($name),
            ", .-",
            stringify!($name),
            "\n"
        ));
    };
}

// Latency chains: load, add 1, store back to the same location.
simple!(k_reg, "", "add rax, 1", "");
simple!(k_lduse, "mov rax, rsi", "mov rax, qword ptr [rax]", "");
simple!(k_lduse_idx, "mov rax, rsi\nxor ecx, ecx", "mov rax, qword ptr [rax + rcx*8]", "");
simple!(k_q_base, "", "mov rax, qword ptr [rsi+16]\nadd rax, 1\nmov qword ptr [rsi+16], rax", "");
simple!(k_d_base, "", "mov eax, dword ptr [rsi+16]\nadd eax, 1\nmov dword ptr [rsi+16], eax", "");
simple!(k_w_base, "", "movzx eax, word ptr [rsi+16]\nadd eax, 1\nmov word ptr [rsi+16], ax", "");
simple!(k_b_base, "", "movzx eax, byte ptr [rsi+16]\nadd eax, 1\nmov byte ptr [rsi+16], al", "");
simple!(k_q_base0, "", "mov rax, qword ptr [rsi]\nadd rax, 1\nmov qword ptr [rsi], rax", "");
simple!(k_q_rsp, "sub rsp, 72", "mov rax, qword ptr [rsp+16]\nadd rax, 1\nmov qword ptr [rsp+16], rax", "add rsp, 72");
simple!(k_w_rsp, "sub rsp, 72", "movzx eax, word ptr [rsp+16]\nadd eax, 1\nmov word ptr [rsp+16], ax", "add rsp, 72");
simple!(k_q_rip, "", "mov rax, qword ptr [rip + rf_static + 64]\nadd rax, 1\nmov qword ptr [rip + rf_static + 64], rax", "");
simple!(k_w_rip, "", "movzx eax, word ptr [rip + rf_static + 64]\nadd eax, 1\nmov word ptr [rip + rf_static + 64], ax", "");
simple!(k_q_fs, "", "mov rax, qword ptr fs:[rf_tls@tpoff + 128]\nadd rax, 1\nmov qword ptr fs:[rf_tls@tpoff + 128], rax", "");
simple!(k_d_fs, "", "mov eax, dword ptr fs:[rf_tls@tpoff + 128]\nadd eax, 1\nmov dword ptr fs:[rf_tls@tpoff + 128], eax", "");
simple!(k_w_fs, "", "movzx eax, word ptr fs:[rf_tls@tpoff + 128]\nadd eax, 1\nmov word ptr fs:[rf_tls@tpoff + 128], ax", "");
simple!(k_b_fs, "", "movzx eax, byte ptr fs:[rf_tls@tpoff + 128]\nadd eax, 1\nmov byte ptr fs:[rf_tls@tpoff + 128], al", "");
simple!(k_q_fsreg, "", "mov rax, qword ptr fs:[rdx + 128]\nadd rax, 1\nmov qword ptr fs:[rdx + 128], rax", "");
simple!(k_q_idx, "mov ecx, 3", "mov rax, qword ptr [rsi + rcx*8 + 16]\nadd rax, 1\nmov qword ptr [rsi + rcx*8 + 16], rax", "");
simple!(k_q_rmw, "", "add qword ptr [rsi+16], 1", "");
simple!(k_d_rmw, "", "add dword ptr [rsi+16], 1", "");
simple!(k_w_rmw, "", "add word ptr [rsi+16], 1", "");
simple!(k_q_rmw_fs, "", "add qword ptr fs:[rf_tls@tpoff + 128], 1", "");
simple!(k_mix_s32_l16, "", "mov dword ptr [rsi+16], eax\nmovzx eax, word ptr [rsi+16]\nadd eax, 1", "");
simple!(k_mix_s64_l32, "", "mov qword ptr [rsi+16], rax\nmov eax, dword ptr [rsi+16]\nadd eax, 1", "");
// Store and load name the same address through different base registers.
simple!(k_q_alias, "mov r8, rsi", "mov rax, qword ptr [rsi+16]\nadd rax, 1\nmov qword ptr [r8+16], rax", "");
// The base register is recomputed every iteration (same value).
simple!(k_q_rebase, "mov r10, rsi", "mov r8, r10\nmov rax, qword ptr [r8+16]\nadd rax, 1\nmov qword ptr [r8+16], rax", "");
// The base register is itself loaded from memory every iteration (heap pointer in TLS).
simple!(k_q_ldbase, "", "mov r8, qword ptr fs:[rf_tls@tpoff + 64]\nmov rax, qword ptr [r8+16]\nadd rax, 1\nmov qword ptr [r8+16], rax", "");
// An unrelated store to a different address sits between the load and the store.
simple!(k_q_between, "lea r8, [rsi+512]", "mov rax, qword ptr [rsi+16]\nadd rax, 1\nmov qword ptr [r8], rdi\nmov qword ptr [rsi+16], rax", "");

// Atomics on an uncontended, L1-resident line.
simple!(k_cmpxchg, "", "mov rax, qword ptr [rsi+16]\nlea rcx, [rax+1]\nlock cmpxchg qword ptr [rsi+16], rcx\njne 9f", "");
simple!(k_xadd, "", "mov eax, 1\nlock xadd qword ptr [rsi+16], rax", "");
simple!(k_xchg, "", "xchg qword ptr [rsi+16], rax", "");
simple!(k_lockadd, "", "lock add qword ptr [rsi+16], 1", "");
simple!(k_lockbts, "", "lock bts qword ptr [rsi+16], 5", "");
// Treiber-stack pop then push of the same node (two CAS).
simple!(
    k_treiber,
    "",
    "mov rax, qword ptr [rsi+16]\nmov rcx, qword ptr [rax]\nlock cmpxchg qword ptr [rsi+16], rcx\njne 9f\nmov r8, rax\nmov rax, qword ptr [rsi+16]\nmov qword ptr [r8], rax\nlock cmpxchg qword ptr [rsi+16], r8\njne 9f",
    ""
);
// Remote-free push with one swap (Vyukov-style MPSC enqueue).
simple!(k_mpsc_push, "lea r8, [rsi+64]", "mov rax, r8\nxchg qword ptr [rsi+16], rax\nmov qword ptr [rax], r8", "");

// Calls.
simple!(k_call, "", "call rf_empty", "");
simple!(k_call2, "", "call rf_empty\ncall rf_empty", "");
simple!(k_icall, "lea rcx, [rip + rf_empty]", "call rcx", "");

// Width probes: independent work per iteration, cycles/iter shows the limit.
simple!(k_alu5, "", "add rax, 1\nadd rcx, 1\nadd rdx, 1\nadd r8, 1\nadd r9, 1", "");
simple!(k_alu7, "", "add rax, 1\nadd rcx, 1\nadd rdx, 1\nadd r8, 1\nadd r9, 1\nadd r10, 1\nadd r11, 1", "");
simple!(k_alu11, "", "add rax, 1\nadd rcx, 1\nadd rdx, 1\nadd r8, 1\nadd r9, 1\nadd r10, 1\nadd r11, 1\nlea rax, [rax+1]\nlea rcx, [rcx+1]\nlea rdx, [rdx+1]\nlea r8, [r8+1]", "");
simple!(k_ld4, "", "mov rax, qword ptr [rsi]\nmov rcx, qword ptr [rsi+8]\nmov rdx, qword ptr [rsi+16]\nmov r8, qword ptr [rsi+24]", "");
simple!(k_ld6, "", "mov rax, qword ptr [rsi]\nmov rcx, qword ptr [rsi+8]\nmov rdx, qword ptr [rsi+16]\nmov r8, qword ptr [rsi+24]\nmov r9, qword ptr [rsi+32]\nmov r10, qword ptr [rsi+40]", "");
simple!(k_ld8, "", "mov rax, qword ptr [rsi]\nmov rcx, qword ptr [rsi+8]\nmov rdx, qword ptr [rsi+16]\nmov r8, qword ptr [rsi+24]\nmov r9, qword ptr [rsi+32]\nmov r10, qword ptr [rsi+40]\nmov r11, qword ptr [rsi+48]\nmov rax, qword ptr [rsi+56]", "");
simple!(k_st2, "", "mov qword ptr [rsi], rax\nmov qword ptr [rsi+8], rax", "");
simple!(k_st4, "", "mov qword ptr [rsi], rax\nmov qword ptr [rsi+8], rax\nmov qword ptr [rsi+16], rax\nmov qword ptr [rsi+24], rax", "");
simple!(k_st6, "", "mov qword ptr [rsi], rax\nmov qword ptr [rsi+8], rax\nmov qword ptr [rsi+16], rax\nmov qword ptr [rsi+24], rax\nmov qword ptr [rsi+32], rax\nmov qword ptr [rsi+40], rax", "");
simple!(k_mix8, "", "mov rax, qword ptr [rsi]\nmov rcx, qword ptr [rsi+8]\nmov qword ptr [rsi+64], r8\nmov qword ptr [rsi+72], r8\nadd r9, 1\nadd r10, 1\nadd r11, 1", "");
// Two taken branches per iteration.
simple!(k_br2, "", "jmp 3f\n3:", "");
simple!(k_br3, "", "jmp 3f\n3:\njmp 4f\n4:", "");
// rseq arm cost alone: the descriptor store every iteration.
simple!(k_rseq_arm, "lea r10, [rip + rf_static]", "mov qword ptr fs:[rdx + 8], 0", "");
simple!(k_rseq_cid, "", "mov eax, dword ptr fs:[rdx + 24]\nshl rax, 12\nadd rax, rsi\nmov rcx, qword ptr [rax]", "");

// Absolute addressing without a segment override (page mapped at 0x20000000).
simple!(k_q_abs, "", "mov rcx, qword ptr ds:[0x20000000]\nadd rcx, 1\nmov qword ptr ds:[0x20000000], rcx", "");
simple!(k_q_fs0base, "", "mov r8, qword ptr fs:[0]\nmov rax, qword ptr [r8 + rf_tls@tpoff + 128]\nadd rax, 1\nmov qword ptr [r8 + rf_tls@tpoff + 128], rax", "");
simple!(k_w_mov16, "", "mov ax, word ptr [rsi+16]\nadd eax, 1\nmov word ptr [rsi+16], ax", "");
simple!(k_lock2, "", "lock add qword ptr [rsi+16], 1\nlock add qword ptr [rsi+256], 1", "");
simple!(k_lock_work, "", "lock add qword ptr [rsi+16], 1\nadd rax, 1\nadd rax, 1\nadd rax, 1\nadd rax, 1\nadd rax, 1\nadd rax, 1\nadd rax, 1\nadd rax, 1\nadd rax, 1\nadd rax, 1\nadd rax, 1\nadd rax, 1\nadd rax, 1\nadd rax, 1\nadd rax, 1\nadd rax, 1", "");
simple!(k_work16, "", "add rax, 1\nadd rax, 1\nadd rax, 1\nadd rax, 1\nadd rax, 1\nadd rax, 1\nadd rax, 1\nadd rax, 1\nadd rax, 1\nadd rax, 1\nadd rax, 1\nadd rax, 1\nadd rax, 1\nadd rax, 1\nadd rax, 1\nadd rax, 1", "");
simple!(k_lock_ldchain, "mov rax, rsi", "lock add qword ptr [rsi+16], 1\nmov rax, qword ptr [rax]\nmov rax, qword ptr [rax]\nmov rax, qword ptr [rax]\nmov rax, qword ptr [rax]", "");
simple!(k_call_work, "", "call rf_empty\nadd rax, 1\nadd rax, 1\nadd rax, 1\nadd rax, 1\nadd rax, 1\nadd rax, 1\nadd rax, 1\nadd rax, 1", "");
simple!(k_work8, "", "add rax, 1\nadd rax, 1\nadd rax, 1\nadd rax, 1\nadd rax, 1\nadd rax, 1\nadd rax, 1\nadd rax, 1", "");

// Dissecting what turns a renamed 1-cycle chain into something slower.
simple!(m1_st_after, "", "mov rax, qword ptr [rsi+16]\nadd rax, 1\nmov qword ptr [rsi+16], rax\nmov qword ptr [rsi+256], rdi", "");
simple!(m2_fsst_after, "", "mov rax, qword ptr [rsi+16]\nadd rax, 1\nmov qword ptr [rsi+16], rax\nmov qword ptr fs:[rdx+8], rdi", "");
simple!(m3_fsld, "", "mov r8d, dword ptr fs:[rdx+24]\nmov rax, qword ptr [rsi+16]\nadd rax, 1\nmov qword ptr [rsi+16], rax", "");
simple!(m4_base_fsld, "", "mov r8d, dword ptr fs:[rdx+24]\nshl r8, 12\nadd r8, rsi\nmov rax, qword ptr [r8+16]\nadd rax, 1\nmov qword ptr [r8+16], rax", "");
simple!(m5_base_alu, "xor r10d, r10d", "mov r8, r10\nshl r8, 12\nadd r8, rsi\nmov rax, qword ptr [r8+16]\nadd rax, 1\nmov qword ptr [r8+16], rax", "");
simple!(m6_base_add, "xor r10d, r10d", "lea r8, [rsi + r10]\nmov rax, qword ptr [r8+16]\nadd rax, 1\nmov qword ptr [r8+16], rax", "");
simple!(m7_base_ld, "", "mov r8, qword ptr [rsi]\nmov rax, qword ptr [r8+16]\nadd rax, 1\nmov qword ptr [r8+16], rax", "");
simple!(m8_base_ldadd, "xor r10d, r10d", "mov r8, qword ptr [rsi]\nadd r8, r10\nmov rax, qword ptr [r8+16]\nadd rax, 1\nmov qword ptr [r8+16], rax", "");
simple!(m9_two_st_after, "", "mov rax, qword ptr [rsi+16]\nadd rax, 1\nmov qword ptr [rsi+16], rax\nmov qword ptr [rsi+256], rdi\nmov qword ptr [rsi+264], rdi", "");
simple!(m10_st_before, "", "mov qword ptr [rsi+256], rdi\nmov rax, qword ptr [rsi+16]\nadd rax, 1\nmov qword ptr [rsi+16], rax", "");
simple!(m11_idx, "mov ecx, 8", "mov rax, qword ptr [rsi+rcx*2]\nadd rax, 1\nmov qword ptr [rsi+rcx*2], rax\nmov qword ptr [rsi+256], rdi", "");

pub type Kernel = unsafe extern "C" fn(u64, *mut u8, u64, *mut u8, u64) -> u64;

macro_rules! decl {
    ($($name:ident),* $(,)?) => {
        extern "C" { $(pub fn $name(iters: u64, state: *mut u8, aux: u64, arr: *mut u8, class: u64) -> u64;)* }
    };
}

decl!(
    p_fl, p_flh, p_arr, p_arrb, p_bm, p_tcm, p_tcmnp, p_rqa, p_rqg, p_rqf, p_mi, c_fl, c_arr, c_tcm,
    c_rqa, c_rqg, c_mi, b_fl, b_flh, b_arr, b_bm, b_tcm, b_tcmnp, b_rqa, b_rqg, b_rqf, b_mi, k_reg,
    k_lduse, k_lduse_idx, k_q_base, k_d_base, k_w_base, k_b_base, k_q_base0, k_q_rsp, k_w_rsp, k_q_rip,
    k_w_rip, k_q_fs, k_d_fs, k_w_fs, k_b_fs, k_q_fsreg, k_q_idx, k_q_rmw, k_d_rmw, k_w_rmw, k_q_rmw_fs,
    k_mix_s32_l16, k_mix_s64_l32, k_q_alias, k_q_rebase, k_q_ldbase, k_q_between, k_cmpxchg, k_xadd,
    k_xchg, k_lockadd, k_lockbts, k_treiber, k_mpsc_push, k_call, k_call2, k_icall, k_alu5, k_alu7,
    k_alu11, k_ld4, k_ld6, k_ld8, k_st2, k_st4, k_st6, k_mix8, k_br2, k_br3, k_rseq_arm, k_rseq_cid,
    p_fl2, p_arr2, p_bm2, p_rqn, p_rqh, p_rq0, p_rqi, c_fl2, c_arr2, c_rqf, b_fl2, b_arr2, b_bm2, b_rq0, b_rqi,
    k_q_abs, k_q_fs0base, k_w_mov16, k_lock2, k_lock_work, k_work16, k_lock_ldchain, k_call_work, k_work8, p_rqd, p_rqc, c_rqd, c_rqc, b_rqd, b_rqc, p_e1, p_e2, p_e3, p_e4, p_e5, p_e7, p_e8, p_e9, p_e10, p_e11, p_e12,
    m1_st_after, m2_fsst_after, m3_fsld, m4_base_fsld, m5_base_alu, m6_base_add, m7_base_ld, m8_base_ldadd, m9_two_st_after, m10_st_before, m11_idx,
);

extern "C" {
    pub fn rf_tls_addr() -> *mut u8;
    pub fn rf_fs0() -> u64;
    pub static mut rf_static: [u64; 32];
    pub static mut rf_abort_count: u64;
    pub static mut rf_slab: [u8; 262144];
}
