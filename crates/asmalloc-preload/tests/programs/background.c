// The allocator's background thread under the preload library (ASMALLOC_BACKGROUND).
//
// usage: background threads     prints how many threads the process has besides the main one,
//                               whether each of them blocks every signal, and the same for a
//                               forked child
//        background exit MS     frees a burst, then exits after MS milliseconds while four
//                               threads allocate inside their caches and an exit handler
//                               allocates: the background thread is asleep or in `maintain`
//        background idle SECS   frees a burst of 200 MiB in random order, then for SECS seconds
//                               does light work that stays inside the allocator's caches, and
//                               prints the anonymous resident memory after the frees and at the
//                               end, in MiB
#include <dirent.h>
#include <pthread.h>
#include <signal.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

static uint64_t next(uint64_t *s) {
    *s ^= *s << 13;
    *s ^= *s >> 7;
    *s ^= *s << 17;
    return *s;
}

// Threads other than the calling one, and how many of them block every signal the C library
// lets a program block (the mask sigfillset gives, less SIGKILL and SIGSTOP, which the kernel
// never blocks).
static void others(int *count, int *blocking) {
    char want[64] = {0};
    {
        sigset_t all;
        sigfillset(&all);
        // SigBlk is the mask in hex, highest signal first, 16 digits for 64 signals.
        uint64_t bits = 0;
        for (int sig = 1; sig <= 64; sig++)
            if (sigismember(&all, sig) == 1 && sig != SIGKILL && sig != SIGSTOP)
                bits |= 1ull << (sig - 1);
        snprintf(want, sizeof want, "%016llx", (unsigned long long)bits);
    }
    *count = 0;
    *blocking = 0;
    pid_t me = getpid();
    DIR *dir = opendir("/proc/self/task");
    if (!dir) abort();
    struct dirent *entry;
    while ((entry = readdir(dir))) {
        if (entry->d_name[0] == '.' || atoi(entry->d_name) == me) continue;
        ++*count;
        char path[128], line[256];
        snprintf(path, sizeof path, "/proc/self/task/%s/status", entry->d_name);
        FILE *status = fopen(path, "r");
        if (!status) continue;
        while (fgets(line, sizeof line, status))
            if (strncmp(line, "SigBlk:", 7) == 0 && strstr(line + 7, want)) ++*blocking;
        fclose(status);
    }
    closedir(dir);
}

static int threads_mode(void) {
    int count, blocking;
    others(&count, &blocking);
    printf("other threads %d, blocking every signal %d\n", count, blocking);
    fflush(stdout);
    pid_t child = fork();
    if (child == 0) {
        for (int i = 0; i < 10000; i++) free(malloc((size_t)(i % 300) + 1));
        others(&count, &blocking);
        printf("child: other threads %d\n", count);
        exit(0);
    }
    int status = 0;
    if (waitpid(child, &status, 0) != child || !WIFEXITED(status) || WEXITSTATUS(status) != 0)
        return 1;
    return 0;
}

// Allocates `n` objects of the twelve sizes from 16 to 256 bytes, writes them, and frees them in
// random order.
static void burst(size_t n) {
    static const size_t sizes[12] = {16, 24, 32, 40, 48, 64, 80, 96, 128, 160, 192, 256};
    uint64_t s = 7;
    void **held = malloc(n * sizeof *held);
    if (!held) abort();
    for (size_t i = 0; i < n; i++) {
        size_t size = sizes[next(&s) % 12];
        held[i] = malloc(size);
        if (!held[i]) abort();
        memset(held[i], 1, size);
    }
    for (size_t i = n - 1; i > 0; i--) {
        size_t j = next(&s) % (i + 1);
        void *t = held[i];
        held[i] = held[j];
        held[j] = t;
    }
    for (size_t i = 0; i < n; i++) free(held[i]);
    free(held);
}

// Light work that stays inside the loaded magazines: a few objects of each of twelve sizes,
// allocated and freed.
static void light(uint64_t *s) {
    static const size_t sizes[12] = {16, 24, 32, 40, 48, 64, 80, 96, 128, 160, 192, 256};
    void *held[120];
    for (int i = 0; i < 120; i++) held[i] = malloc(sizes[next(s) % 12]);
    for (int i = 0; i < 120; i++) free(held[i]);
}

static volatile int stop;

static void *worker(void *arg) {
    uint64_t s = (uint64_t)(uintptr_t)arg | 1;
    while (!stop) light(&s);
    return NULL;
}

static void at_exit(void) {
    for (int i = 0; i < 1000; i++) free(malloc((size_t)i * 64 + 1));
}

static long anon_kib(void) {
    FILE *status = fopen("/proc/self/status", "r");
    char line[256];
    long kib = -1;
    while (status && fgets(line, sizeof line, status))
        if (strncmp(line, "RssAnon:", 8) == 0) kib = atol(line + 8);
    if (status) fclose(status);
    return kib;
}

static void sleep_ms(long ms) {
    struct timespec t = {ms / 1000, (ms % 1000) * 1000000};
    while (nanosleep(&t, &t) != 0) {
    }
}

int main(int argc, char **argv) {
    if (argc > 1 && strcmp(argv[1], "threads") == 0) return threads_mode();
    if (argc > 2 && strcmp(argv[1], "exit") == 0) {
        atexit(at_exit);
        burst(200000);
        pthread_t t[4];
        for (long i = 0; i < 4; i++) pthread_create(&t[i], NULL, worker, (void *)(i + 1));
        sleep_ms(atol(argv[2]));
        puts("exit while maintaining");
        exit(0);
    }
    if (argc > 2 && strcmp(argv[1], "idle") == 0) {
        long secs = atol(argv[2]);
        size_t n = 1600000;
        uint64_t s = 3;
        burst(n);
        long freed = anon_kib();
        struct timespec start, now;
        clock_gettime(CLOCK_MONOTONIC, &start);
        do {
            for (int i = 0; i < 16; i++) light(&s);
            sleep_ms(1);
            clock_gettime(CLOCK_MONOTONIC, &now);
        } while (now.tv_sec - start.tv_sec < secs);
        printf("freed %ld end %ld\n", freed / 1024, anon_kib() / 1024);
        return 0;
    }
    fprintf(stderr, "usage: background threads | exit MS | idle SECS\n");
    return 2;
}
