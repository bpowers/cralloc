// Frees a pointer that no allocator handed out: outside the heap, or (with `interior`) into
// the middle of a large block. Under cralloc this must abort (SIGABRT) before the program can
// print its last line. A pointer into the middle of a small block is the caller's bug that the
// fast path does not look for (docs/design.md, "Interfaces"), so it is not tried here for free.
// `realloc` checks every pointer it takes: with `realloc-interior` it is given one into the
// middle of a small block, and with `realloc-outside` one outside the heap, and must abort too.
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static char not_a_block[64];

int main(int argc, char **argv) {
    void *p = not_a_block + 16;
    const char *mode = argc > 1 ? argv[1] : "";
    if (strcmp(mode, "interior") == 0) {
        // An interior pointer of a large block, which the slow path checks.
        p = (char *)malloc(100000) + 8;
    } else if (strncmp(mode, "realloc", 7) == 0) {
        void *q = strcmp(mode, "realloc-interior") == 0 ? (char *)malloc(100) + 16 : p;
        fflush(stdout);
        q = realloc(q, 300);
        printf("realloc returned %p\n", q);
        return 0;
    }
    fflush(stdout);
    free(p);
    puts("free returned");
    return 0;
}
