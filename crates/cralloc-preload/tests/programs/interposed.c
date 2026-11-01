// Prints which allocator serves this process's malloc: "cralloc <version>" when cralloc's
// `cralloc_version` is defined and malloc behaves as cralloc's does (malloc(24) is a 32-byte
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
    const char *(*version)(void) = (const char *(*)(void))dlsym(RTLD_DEFAULT, "cralloc_version");
    if (usable == 32 && version != NULL)
        printf("cralloc %s\n", version());
    else
        puts("other");
    return 0;
}
