// Prints which allocator serves this process's malloc: "asmalloc <version>" when asmalloc's
// `asmalloc_version` is defined and malloc behaves as asmalloc's does (malloc(24) is a 32-byte
// class, where glibc reports 24 usable bytes), else "other".
#define _GNU_SOURCE
#include <dlfcn.h>
#include <malloc.h>
#include <stdio.h>
#include <stdlib.h>

int main(void) {
    void *p = malloc(24);
    size_t usable = malloc_usable_size(p);
    free(p);
    const char *(*version)(void) = (const char *(*)(void))dlsym(RTLD_DEFAULT, "asmalloc_version");
    if (usable == 32 && version != NULL)
        printf("asmalloc %s\n", version());
    else
        puts("other");
    return 0;
}
