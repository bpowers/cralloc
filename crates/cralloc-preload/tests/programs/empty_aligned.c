// Resident memory after a zero-size block aligned to a huge page or more is freed in a slot
// that an earlier block left resident. The block's recorded size is zero; the slot must go back
// to malloc_trim all the same, as it does with a size of one. (The unit tests are
// `back::tests::large_charge` and `heap::address::tests`.)
// usage: empty_aligned [SIZE]    with the library preloaded; SIZE defaults to 0
#define _GNU_SOURCE
#include <malloc.h>
#include <stdio.h>
#include <stdlib.h>
#include <unistd.h>

static long rss_kib(void) {
    long pages = 0, rss = 0;
    FILE *f = fopen("/proc/self/statm", "r");
    if (f && fscanf(f, "%ld %ld", &pages, &rss) != 2) rss = 0;
    if (f) fclose(f);
    return rss * (sysconf(_SC_PAGESIZE) / 1024);
}

int main(int argc, char **argv) {
    size_t align = (size_t)64 << 20;
    size_t size = argc > 1 ? strtoul(argv[1], 0, 0) : 0;
    long base = rss_kib();
    volatile char *full = aligned_alloc(align, align);
    for (size_t i = 0; i < align; i += 4096) full[i] = 1;
    printf("written:        +%ld KiB\n", rss_kib() - base);
    free((void *)full);
    void *again = aligned_alloc(align, size);
    printf("same slot:      %s (size %zu)\n", again == (void *)full ? "yes" : "no", size);
    free(again);
    malloc_trim(0);
    printf("after a trim:   +%ld KiB\n", rss_kib() - base);
    // Past the decay interval, with a slow path to beat the heart.
    sleep(3);
    free(malloc(100000));
    malloc_trim(0);
    printf("3 s and a trim: +%ld KiB\n", rss_kib() - base);
    return 0;
}
