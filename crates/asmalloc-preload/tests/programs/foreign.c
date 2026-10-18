// Frees a pointer that no allocator handed out: outside the heap, or (with `interior`) into
// the middle of a large block. Under asmalloc this must abort (SIGABRT) before the program can
// print its last line. A pointer into the middle of a small block is the caller's bug that the
// fast path does not look for (docs/design.md, "Interfaces"), so it is not tried here.
#include <stdio.h>
#include <stdlib.h>

static char not_a_block[64];

int main(int argc, char **argv) {
    void *p = not_a_block + 16;
    if (argc > 1) {
        // An interior pointer of a large block, which the slow path checks.
        p = (char *)malloc(100000) + 8;
    }
    fflush(stdout);
    free(p);
    puts("free returned");
    return 0;
}
