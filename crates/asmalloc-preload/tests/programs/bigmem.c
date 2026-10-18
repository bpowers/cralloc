// Peak memory of simple large-block patterns.
// usage: bigmem chain|grow|huge|hugecalloc ROUNDS [THREADS [TOP_MIB]]
//   chain: realloc a block from 64 bytes up to TOP_MIB (256) by factors of 1.05 to 2, writing
//          every page of the new room, then back down; ROUNDS times on each of THREADS threads.
//   grow:  as chain, but never shrinks: free at the top.
//   huge:  malloc 512 MiB, write one byte per MiB, free.
//   hugecalloc: the same with calloc.
// Prints the process's peak resident memory, what is resident once every block is freed, and
// what is left after malloc_trim. ALIGN=n starts each chain with memalign(n, 64); BUSY=1 runs
// five seconds of small allocations after the frees, so that an allocator that returns memory
// from its slow paths has some to run on, and prints what is resident then.
// Compare an allocator with glibc: peak memory well above glibc's is the finding.
#include <malloc.h>
#include <pthread.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/resource.h>
#include <time.h>

static const char *mode;
static int rounds;
static size_t top;

static long rss_mib(void) {
    long pages = 0, rss = 0;
    FILE *f = fopen("/proc/self/statm", "r");
    if (f && fscanf(f, "%ld %ld", &pages, &rss) != 2) rss = 0;
    if (f) fclose(f);
    return rss * 4096 / (1 << 20);
}

static long peak_mib(void) {
    struct rusage u;
    getrusage(RUSAGE_SELF, &u);
    return u.ru_maxrss / 1024;
}

static void *work(void *arg) {
    unsigned long long seed = (unsigned long long)(size_t)arg;
    for (int r = 0; r < rounds; r++) {
        if (!strcmp(mode, "chain") || !strcmp(mode, "grow")) {
            size_t n = 64;
            const char *al = getenv("ALIGN");
            char *p = al ? memalign(strtoul(al, NULL, 0), n) : malloc(n);
            memset(p, 1, n);
            while (n < top) {
                seed = seed * 6364136223846793005ull + 1442695040888963407ull;
                size_t m = n + n * (5 + (seed >> 33) % 96) / 100 + 1;
                if (m > top) m = top;
                p = realloc(p, m);
                if (!p) {
                    printf("realloc to %zu failed\n", m);
                    exit(1);
                }
                for (size_t i = n; i < m; i += 4096) p[i] = 2;
                n = m;
            }
            if (!strcmp(mode, "chain"))
                while (n > 64) {
                    n /= 2;
                    p = realloc(p, n);
                }
            free(p);
        } else {
            size_t n = 512u << 20;
            char *p = !strcmp(mode, "huge") ? malloc(n) : calloc(1, n);
            for (size_t i = 0; i < n; i += 1 << 20) p[i] = 3;
            free(p);
        }
    }
    return NULL;
}

int main(int argc, char **argv) {
    mode = argc > 1 ? argv[1] : "chain";
    rounds = argc > 2 ? atoi(argv[2]) : 3;
    int threads = argc > 3 ? atoi(argv[3]) : 1;
    top = (size_t)(argc > 4 ? atoi(argv[4]) : 256) << 20;
    pthread_t t[256];
    for (int i = 0; i < threads; i++) pthread_create(&t[i], NULL, work, (void *)(size_t)(i + 1));
    for (int i = 0; i < threads; i++) pthread_join(t[i], NULL);
    long after = rss_mib();
    // Five seconds of small allocations, so the heartbeat has slow paths to run on.
    long later = -1;
    if (getenv("BUSY")) {
        struct timespec t0, t1;
        clock_gettime(CLOCK_MONOTONIC, &t0);
        void *keep[4096] = {0};
        unsigned long long x = 1;
        do {
            for (int i = 0; i < 100000; i++) {
                x = x * 6364136223846793005ull + 1;
                int k = (int)(x >> 52);
                free(keep[k]);
                keep[k] = malloc((x >> 33) % 30000 + 1);
            }
            clock_gettime(CLOCK_MONOTONIC, &t1);
        } while (t1.tv_sec - t0.tv_sec < 5);
        for (int i = 0; i < 4096; i++) free(keep[i]);
        later = rss_mib();
        printf("after 5 s of small allocations: %ld MiB\n", later);
    }
    malloc_trim(0);
    printf("%s, %d rounds on %d threads to %zu MiB: peak %ld MiB, %ld MiB after all freed, %ld "
           "after malloc_trim\n",
           mode, rounds, threads, top >> 20, peak_mib(), after, rss_mib());
    return 0;
}
