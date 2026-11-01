// How many buffers grown by doubling fit the heap's address space before an allocation fails,
// for the headroom a growing block gets (docs/research/memory.md, "Headroom for growing
// blocks"). Run it preloaded under an address-space limit, which shrinks cralloc's regions:
//
//   prlimit --as=$((32 << 30)) env LD_PRELOAD=libcralloc_preload.so grow_exhaust 1048576
//
// Each buffer starts at 64 KiB and doubles by realloc to FINAL bytes, and is kept. Only its
// first byte is written, so a move copies nothing (the move leaves out zero pages) and the
// buffers cost address space, not memory. Stops at the first null, and prints how many
// buffers were whole, the size whose allocation failed, and whether a plain malloc of that
// size and of 64 KiB succeed then, with the buffer that failed to grow still held.
#include <stdio.h>
#include <stdlib.h>

int main(int argc, char **argv) {
  size_t final = argc > 1 ? strtoull(argv[1], 0, 0) : 1 << 20;
  size_t most = argc > 2 ? strtoull(argv[2], 0, 0) : 1 << 22;
  char **kept = malloc(most * sizeof *kept);
  if (!kept) return 1;
  size_t n = 0, failed_at = 0;
  for (; n < most; n++) {
    size_t size = 64 << 10;
    char *p = malloc(size);
    if (!p) { failed_at = size; break; }
    p[0] = 1;
    while (size < final) {
      char *q = realloc(p, size * 2);
      if (!q) { failed_at = size * 2; break; }
      p = q;
      size *= 2;
    }
    if (failed_at) {
      // With the buffer that could not grow still held: would a plain block of the size it
      // asked for have been there?
      // Written, so that the compiler cannot assume they succeed and leave the calls out.
      char *volatile plain = malloc(failed_at), *volatile small = malloc(64 << 10);
      if (plain) plain[0] = 1;
      if (small) small[0] = 1;
      printf("final=%zu whole=%zu live_mib=%zu failed_at=%zu malloc_that_size=%s malloc_64k=%s\n",
             final, n, (n * final) >> 20, failed_at, plain ? "ok" : "null", small ? "ok" : "null");
      return 0;
    }
    kept[n] = p;
  }
  printf("final=%zu whole=%zu live_mib=%zu no failure\n", final, n, (n * final) >> 20);
  return 0;
}
