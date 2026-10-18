/* The free and malloc chains of an incumbent allocator, linked statically, against placement.
 *
 *   incumbent-P [K]
 *
 * build-incumbent.sh links one binary per placement P, with the allocator's free and malloc
 * functions at P bytes past a 64-byte boundary (their instructions unchanged). Each binary
 * prints cycles per operation, read with rdpmc around each phase so that a phase of K
 * operations is timed without a system call inside it:
 *   free    K frees in a row (the objects allocated just before, untimed)
 *   malloc  K mallocs in a row
 *   pair    malloc then free, K times
 * and user-mode op-cache misses per operation over the free phases. Pinned to RF_CPU (2).
 */
#define _GNU_SOURCE
#include <linux/perf_event.h>
#include <sched.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/syscall.h>
#include <unistd.h>

void *ALLOC_MALLOC(size_t);
void ALLOC_FREE(void *);

struct ctr {
    int fd;
    struct perf_event_mmap_page *pc;
};

static struct ctr open_ctr(uint32_t type, uint64_t config) {
    struct perf_event_attr a;
    memset(&a, 0, sizeof a);
    a.size = sizeof a;
    a.type = type;
    a.config = config;
    a.exclude_kernel = 1;
    a.exclude_hv = 1;
    struct ctr c;
    c.fd = syscall(SYS_perf_event_open, &a, 0, -1, -1, 0);
    if (c.fd < 0) {
        perror("perf_event_open");
        exit(1);
    }
    c.pc = mmap(NULL, 4096, PROT_READ, MAP_SHARED, c.fd, 0);
    if (c.pc == MAP_FAILED || !c.pc->cap_user_rdpmc || !c.pc->index) {
        fprintf(stderr, "rdpmc not available\n");
        exit(1);
    }
    return c;
}

static inline uint64_t rd(struct ctr *c) {
    uint32_t lo, hi;
    __asm__ volatile("rdpmc" : "=a"(lo), "=d"(hi) : "c"(c->pc->index - 1));
    return ((uint64_t)hi << 32 | lo) & ((1ull << c->pc->pmc_width) - 1);
}

int main(int argc, char **argv) {
    int k = argc > 1 ? atoi(argv[1]) : 256;
    int cpu = getenv("RF_CPU") ? atoi(getenv("RF_CPU")) : 2;
    int reps = 4000;
    cpu_set_t s;
    CPU_ZERO(&s);
    CPU_SET(cpu, &s);
    sched_setaffinity(0, sizeof s, &s);
    struct ctr cyc = open_ctr(PERF_TYPE_HARDWARE, PERF_COUNT_HW_CPU_CYCLES);
    struct ctr oc = open_ctr(PERF_TYPE_RAW, 0x20000048full);
    void **p = calloc(k, sizeof *p);
    double best[3] = {1e30, 1e30, 1e30};
    uint64_t ocm = 0, nfree = 0;
    for (int round = 0; round < 5; round++) {
        uint64_t t[3] = {0, 0, 0};
        for (int r = 0; r < reps; r++) {
            uint64_t a = rd(&cyc);
            for (int i = 0; i < k; i++) {
                p[i] = ALLOC_MALLOC(32);
                __asm__ volatile("" ::"r"(p[i]) : "memory");
            }
            uint64_t b = rd(&cyc), o0 = rd(&oc);
            for (int i = 0; i < k; i++)
                ALLOC_FREE(p[i]);
            uint64_t c = rd(&cyc), o1 = rd(&oc);
            for (int i = 0; i < k; i++) {
                void *q = ALLOC_MALLOC(32);
                __asm__ volatile("" ::"r"(q) : "memory");
                ALLOC_FREE(q);
            }
            uint64_t d = rd(&cyc);
            t[1] += b - a;
            t[0] += c - b;
            t[2] += d - c;
            if (round > 0) {
                ocm += o1 - o0;
                nfree += k;
            }
        }
        for (int j = 0; j < 3; j++) {
            double v = (double)t[j] / ((double)reps * k);
            if (round > 0 && v < best[j])
                best[j] = v;
        }
    }
    printf("free %.3f malloc %.3f pair %.3f ocmiss/free %.3f free@%lu malloc@%lu\n", best[0], best[1], best[2],
           (double)ocm / nfree, (unsigned long)(uintptr_t)ALLOC_FREE % 64, (unsigned long)(uintptr_t)ALLOC_MALLOC % 64);
    return 0;
}
