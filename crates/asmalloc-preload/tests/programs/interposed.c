// Prints which allocator serves this process's malloc. asmalloc puts malloc(24) in a 32-byte
// class (blocks of 16 bytes or more are 16-aligned), and glibc reports 24 usable bytes.
#include <malloc.h>
#include <stdio.h>
#include <stdlib.h>

int main(void) {
    void *p = malloc(24);
    size_t usable = malloc_usable_size(p);
    free(p);
    puts(usable == 32 ? "asmalloc" : "other");
    return 0;
}
