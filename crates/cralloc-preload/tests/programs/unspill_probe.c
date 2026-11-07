// What an in-place growth of a staggered large block costs, call by call: the faults, huge-page
// faults and TSC ticks inside realloc, the resident memory it adds before the program writes a
// byte, and the same for the program's fill of the new room (Back::unspill). A buffer is grown
// by doubling from 1 MiB to the top size, filled as it grows, and freed, over and over; a second
// buffer holds slot 0 of the class, which has no stagger, so the one measured sits in slot 1.
// Every byte is checked after each realloc.
//
// The free of a block advised for huge pages returns its slot whole, so every repetition is a
// fresh one; the resident case is a slot a move kept (back/tests/unspill.rs).
//
// Run by hand; it is in no test. Fault counts are exact; ticks are only as good as the core
// is quiet.
// usage: scripts/memlimit -m 1G -- env LD_PRELOAD=libcralloc_preload.so \
//            unspill_probe [top MiB, default 8] [repetitions, default 4] [nothp]
// nothp turns transparent huge pages off for the process (PR_SET_THP_DISABLE), as a system set
// to "never" has them.
#define _GNU_SOURCE
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/prctl.h>
#include <sys/resource.h>
#include <x86intrin.h>

#define M ((size_t)1 << 20)

static long field(const char *path, const char *key) {
    static char line[256];
    long v = -1;
    FILE *f = fopen(path, "r");
    if (!f) return -1;
    while (fgets(line, sizeof line, f))
        if (!strncmp(line, key, strlen(key))) { sscanf(line + strlen(key), "%ld", &v); break; }
    fclose(f);
    return v;
}

static long minflt(void) {
    struct rusage r;
    getrusage(RUSAGE_SELF, &r);
    return r.ru_minflt;
}

static long rss_kib(void) { return field("/proc/self/smaps_rollup", "Rss:"); }
// System-wide, so another process's huge-page faults can show up here.
static long thp(void) { return field("/proc/vmstat", "thp_fault_alloc "); }

static unsigned char byte_at(size_t i) { return (unsigned char)(i / 7 + i / 4096) | 1; }

static void fill(char *p, size_t from, size_t to) {
    for (size_t i = from; i < to; i++) p[i] = (char)byte_at(i);
}

static void check(const char *p, size_t len, const char *when) {
    for (size_t i = 0; i < len; i++)
        if ((unsigned char)p[i] != byte_at(i)) {
            printf("LOST: byte %zu of %zu %s\n", i, len, when);
            exit(1);
        }
}

int main(int argc, char **argv) {
    size_t top = (argc > 1 ? strtoul(argv[1], 0, 0) : 8) * M;
    int reps = argc > 2 ? atoi(argv[2]) : 4;
    if (argc > 3 && !strcmp(argv[3], "nothp") && prctl(PR_SET_THP_DISABLE, 1, 0, 0, 0)) {
        perror("PR_SET_THP_DISABLE");
        return 1;
    }
    // Warm stdio and the files read below, so that they do not fault inside a measurement.
    printf("top %zu MiB\n", top / M);
    (void)rss_kib();
    (void)thp();
    char *a = malloc(M);
    fill(a, 0, M);
    a = realloc(a, 2 * M);
    fill(a, M, 2 * M);
    for (int rep = 0; rep < reps; rep++) {
        char *b = malloc(M);
        fill(b, 0, M);
        b = realloc(b, 2 * M);
        fill(b, M, 2 * M);
        printf("repetition %d, block %p, stagger %zu\n", rep, (void *)b, (size_t)b % 4096);
        for (size_t size = 2 * M; size < top; size *= 2) {
            long f0 = minflt(), h0 = thp(), r0 = rss_kib();
            uint64_t t0 = __rdtsc();
            char *c = realloc(b, 2 * size);
            uint64_t t1 = __rdtsc();
            long f1 = minflt(), h1 = thp(), r1 = rss_kib();
            if (c != b) printf("  moved at %zu MiB\n", size / M);
            b = c;
            check(b, size, "after realloc");
            long f2 = minflt(), h2 = thp();
            uint64_t t2 = __rdtsc();
            fill(b, size, 2 * size);
            uint64_t t3 = __rdtsc();
            long f3 = minflt(), h3 = thp();
            printf("  %2zu->%2zu MiB: realloc %4ld faults %2ld huge %7lu ticks, rss %+6ld KiB;"
                   " fill %4ld faults %2ld huge %8lu ticks\n",
                   size / M, 2 * size / M, f1 - f0, h1 - h0, (unsigned long)(t1 - t0), r1 - r0,
                   f3 - f2, h3 - h2, (unsigned long)(t3 - t2));
        }
        check(b, top, "before free");
        free(b);
    }
    check(a, 2 * M, "in the block held throughout");
    free(a);
    return 0;
}
