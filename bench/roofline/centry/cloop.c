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

#ifdef __aarch64__
#include <dirent.h>

/* Reads a small file under the PMU's sysfs directory into buf; returns 0 if it cannot. */
static int pmu_read(const char *pmu, const char *file, char *buf, size_t n) {
    char path[256];
    snprintf(path, sizeof path, "/sys/bus/event_source/devices/%s/%s", pmu, file);
    FILE *f = fopen(path, "r");
    if (!f) return 0;
    size_t got = fread(buf, 1, n - 1, f);
    buf[got] = 0;
    fclose(f);
    return got > 0;
}

/* Whether a kernel CPU list such as "2-5,8" holds cpu. */
static int in_list(const char *list, int cpu) {
    while (*list) {
        char *end;
        long a = strtol(list, &end, 10), b = a;
        if (*end == '-') b = strtol(end + 1, &end, 10);
        if (a <= cpu && cpu <= b) return 1;
        list = *end == ',' ? end + 1 : "";
    }
    return 0;
}

/* The PMU of the CPU this runs on: on a machine with two kinds of core the generic hardware type
 * lands on whichever the kernel picks. Cycles and instructions by that PMU's own codes. */
static int counter_on(int cpu, uint64_t which) {
    DIR *d = opendir("/sys/bus/event_source/devices");
    struct dirent *e;
    while (d && (e = readdir(d))) {
        char buf[64];
        if (!pmu_read(e->d_name, "cpus", buf, sizeof buf) || !in_list(buf, cpu)) continue;
        const char *names[2][2] = {{"events/cycles", "events/cpu_cycles"}, {"events/instructions", "events/inst_retired"}};
        int w = which == PERF_COUNT_HW_INSTRUCTIONS;
        unsigned long code;
        char ev[64];
        if (!pmu_read(e->d_name, names[w][0], ev, sizeof ev) && !pmu_read(e->d_name, names[w][1], ev, sizeof ev)) continue;
        if (sscanf(ev, "event=%lx", &code) != 1 || !pmu_read(e->d_name, "type", buf, sizeof buf)) continue;
        closedir(d);
        struct perf_event_attr a;
        memset(&a, 0, sizeof a);
        a.size = sizeof a;
        a.type = (uint32_t)strtoul(buf, NULL, 10);
        a.config = code;
        a.disabled = 1;
        a.exclude_kernel = 1;
        a.exclude_hv = 1;
        return (int)syscall(SYS_perf_event_open, &a, 0, -1, -1, 0);
    }
    if (d) closedir(d);
    return -1;
}
#endif

static int counter(uint64_t config) {
#ifdef __aarch64__
    return counter_on(sched_getcpu(), config);
#endif
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

/* As rust-alloc's churn: free a random one of 10,000 live blocks, allocate one of another size. */
static __attribute__((noinline)) void churn(uint64_t n) {
    enum { LIVE = 10000 };
    static void *live[LIVE];
    uint64_t x = 0x9E3779B97F4A7C15ull;
#define NEXT() (x ^= x << 13, x ^= x >> 7, x ^= x << 17, x)
#define SIZE(r) (16 + (size_t)(((r) >> 20) % 63) * 8)
    for (int i = 0; i < LIVE; i++) live[i] = malloc(SIZE(NEXT()));
    for (uint64_t i = 0; i < n; i++) {
        uint64_t r = NEXT();
        uint64_t at = r % LIVE;
        free(live[at]);
        void *p = malloc(SIZE(r));
        *(uint64_t *)p = r;
        live[at] = p;
    }
    for (int i = 0; i < LIVE; i++) free(live[i]);
#undef NEXT
#undef SIZE
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
            else if (!strcmp(form, "churn")) churn(iters);
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
