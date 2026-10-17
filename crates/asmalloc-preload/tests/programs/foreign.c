// Frees a pointer that no allocator handed out. Under asmalloc this must abort (SIGABRT)
// before the program can print its last line.
#include <stdio.h>
#include <stdlib.h>

static char not_a_block[64];

int main(int argc, char **argv) {
    void *p = not_a_block + 16;
    if (argc > 1) {
        // An interior pointer of a real block.
        p = (char *)malloc(100) + 8;
    }
    fflush(stdout);
    free(p);
    puts("free returned");
    return 0;
}
