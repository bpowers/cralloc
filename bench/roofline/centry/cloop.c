/* The C side of the comparison: plain C loops calling malloc and free through whatever
 * the build gives them (PLT, IBT PLT or -fno-plt), run with each allocator preloaded.
 *
 *   cloop FORM...      FORM: pair:N, burst:K:N, mixed:LO:HI, calloc:N, realloc:N
 *
 * prints, per form, user-space cycles and instructions per allocate+free (per element for
 * bursts). Pins itself to RF_CPU (default 13); RF_ITERS (default 10M) operations per run;
 * RF_REPS (default 5) runs, the first a warm-up, the fastest reported.
 */
#define _GNU_SOURCE
#include <linux/perf_event.h>
#include <sched.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/syscall.h>
#include <unistd.h>

static int fd_cyc = -1, fd_ins = -1;

static int counter(uint64_t config) {
    struct perf_event_attr a;
    memset(&a, 0, sizeof a);
    a.size = sizeof a;
    a.type = PERF_TYPE_HARDWARE;
    a.config = config;
    a.disabled = 1;
    a.exclude_kernel = 1;
    a.exclude_hv = 1;
    return (int)syscall(SYS_perf_event_open, &a, 0, -1, -1, 0);
}

static void start(void) {
    ioctl(fd_cyc, PERF_EVENT_IOC_RESET, 0);
    ioctl(fd_ins, PERF_EVENT_IOC_RESET, 0);
    ioctl(fd_cyc, PERF_EVENT_IOC_ENABLE, 0);
    ioctl(fd_ins, PERF_EVENT_IOC_ENABLE, 0);
}

static void stop(uint64_t *cyc, uint64_t *ins) {
    ioctl(fd_ins, PERF_EVENT_IOC_DISABLE, 0);
    ioctl(fd_cyc, PERF_EVENT_IOC_DISABLE, 0);
    if (read(fd_cyc, cyc, 8) != 8) *cyc = 0;
    if (read(fd_ins, ins, 8) != 8) *ins = 0;
}

/* Keeps the compiler from folding a malloc and its free together. */
#define ESCAPE(p) __asm__ volatile("" : : "r"(p) : "memory")

static uint64_t env_u(const char *k, uint64_t dflt) {
    const char *v = getenv(k);
    return v ? strtoull(v, NULL, 0) : dflt;
}

static __attribute__((noinline)) void pair(uint64_t n, size_t size) {
    for (uint64_t i = 0; i < n; i++) {
        void *p = malloc(size);
        ESCAPE(p);
        free(p);
    }
}

static __attribute__((noinline)) void burst(uint64_t n, unsigned k, size_t size) {
    void *ps[256];
    for (uint64_t i = 0; i < n; i++) {
        for (unsigned j = 0; j < k; j++) ps[j] = malloc(size);
        ESCAPE(ps);
        for (unsigned j = 0; j < k; j++) free(ps[j]);
    }
}

static uint32_t sizes[64];

static __attribute__((noinline)) void mixed(uint64_t n) {
    for (uint64_t i = 0; i < n; i++) {
        void *p = malloc(sizes[i & 63]);
        ESCAPE(p);
        free(p);
    }
}

static __attribute__((noinline)) void callocs(uint64_t n, size_t size) {
    for (uint64_t i = 0; i < n; i++) {
        void *p = calloc(1, size);
        ESCAPE(p);
        free(p);
    }
}

/* Grow a block in place within its class, then free it: realloc's cheapest case. */
static __attribute__((noinline)) void reallocs(uint64_t n, size_t size) {
    for (uint64_t i = 0; i < n; i++) {
        void *p = malloc(size);
        ESCAPE(p);
        p = realloc(p, size + 1);
        ESCAPE(p);
        free(p);
    }
}

int main(int argc, char **argv) {
    int cpu = (int)env_u("RF_CPU", 13);
    cpu_set_t set;
    CPU_ZERO(&set);
    CPU_SET(cpu, &set);
    if (sched_setaffinity(0, sizeof set, &set)) {
        perror("sched_setaffinity");
        return 1;
    }
    fd_cyc = counter(PERF_COUNT_HW_CPU_CYCLES);
    fd_ins = counter(PERF_COUNT_HW_INSTRUCTIONS);
    if (fd_cyc < 0 || fd_ins < 0) {
        perror("perf_event_open");
        return 1;
    }
    uint64_t iters = env_u("RF_ITERS", 10000000), reps = env_u("RF_REPS", 5);
    for (int a = 1; a < argc; a++) {
        char form[32] = "";
        unsigned x = 0, y = 0;
        sscanf(argv[a], "%31[a-z]:%u:%u", form, &x, &y);
        double best = 1e300, best_ins = 0;
        for (uint64_t r = 0; r < reps; r++) {
            uint64_t cyc, ins, ops = iters;
            if (!strcmp(form, "mixed")) {
                srand(1);
                for (int i = 0; i < 64; i++) sizes[i] = x + (uint32_t)(rand() % (y - x + 1));
            }
            start();
            if (!strcmp(form, "pair")) pair(iters, x);
            else if (!strcmp(form, "burst")) burst(iters / x, x, y), ops = iters / x * x;
            else if (!strcmp(form, "mixed")) mixed(iters);
            else if (!strcmp(form, "calloc")) callocs(iters, x);
            else if (!strcmp(form, "realloc")) reallocs(iters, x);
            else {
                fprintf(stderr, "unknown form %s\n", argv[a]);
                return 2;
            }
            stop(&cyc, &ins);
            if ((r > 0 || reps == 1) && (double)cyc / ops < best) {
                best = (double)cyc / ops;
                best_ins = (double)ins / ops;
            }
        }
        printf("%-16s %8.2f %8.2f\n", argv[a], best, best_ins);
    }
    return 0;
}
