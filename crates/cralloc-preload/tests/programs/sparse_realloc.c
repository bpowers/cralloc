// Resident memory after moving a sparsely written large block with realloc: a 64 MiB block
// with one byte written per MiB, grown to 256 MiB (which moves it), then shrunk back. glibc
// moves such blocks with mremap, which keeps the pages that were never written unmapped; a
// realloc that copies the whole block writes them all and makes the new block resident.
// usage: sparse_realloc [MIB]
#include <stdio.h>
#include <stdlib.h>

static long rss_mib(void) {
    long pages = 0, rss = 0;
    FILE *f = fopen("/proc/self/statm", "r");
    if (f && fscanf(f, "%ld %ld", &pages, &rss) != 2) rss = 0;
    if (f) fclose(f);
    return rss * 4096 / (1 << 20);
}

int main(int argc, char **argv) {
    size_t n = (size_t)(argc > 1 ? atoi(argv[1]) : 64) << 20;
    char *p = malloc(n);
    for (size_t i = 0; i < n; i += 1 << 20) p[i] = 1;
    long before = rss_mib();
    p = realloc(p, 4 * n);
    if (!p) return 1;
    long grown = rss_mib();
    for (size_t i = 0; i < n; i += 1 << 20)
        if (p[i] != 1) return 2;
    p = realloc(p, n / 8);
    long shrunk = rss_mib();
    free(p);
    printf("written %zu MiB sparsely: resident %ld MiB, %ld after growing it 4x, %ld after "
           "shrinking it to an eighth\n",
           n >> 20, before, grown, shrunk);
    return 0;
}
