// The allocator's background thread under the preload library (CRALLOC_BACKGROUND).
//
// usage: background threads     prints how many threads the process has besides the main one,
//                               whether each of them blocks every signal, and the same for a
//                               forked child
//        background exit MS     frees a burst, then exits after MS milliseconds while four
//                               threads allocate inside their caches and an exit handler
//                               allocates: the background thread is asleep or in `maintain`
//        background idle SECS   frees a burst of 200 MiB in random order, then for SECS seconds
//                               does light work, one object at a time, that never leaves the
//                               allocator's caches, and prints the anonymous resident memory
//                               after the frees and at the end, in MiB
//        background spawn       creates and joins one thread, then prints how many threads
//                               named `cralloc` the process has, and the same for a forked
//                               child that creates one of its own
//        background rewrite     writes over its initial environment's strings and sets
//                               CRALLOC_BACKGROUND=1 with setenv, then creates and joins one
//                               thread and prints how many threads named `cralloc` it has
//        background quiet SECS  idle, in a process that created a thread first, so that the
//                               allocator's default thread exists
//        background silent SECS creates a thread, frees a burst, and then does nothing at all
//                               for SECS seconds: what the allocator's thread does in a
//                               silent process is all that happens
//        background trickle SECS
//                               idle, plus a few slow paths every half second (40 blocks of
//                               8 KiB held together and freed), as a server answering a
//                               health check makes
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

// Threads other than the calling one, named `name` if it is not null, and how many of them
// block every signal the C library lets a program block (the mask sigfillset gives, less
// SIGKILL and SIGSTOP, which the kernel never blocks). A thread may block more: glibc blocks
// everything while a new thread starts, and while a joined one finishes leaving.
static void others_named(const char *name, int *count, int *blocking) {
    uint64_t want = 0;
    {
        sigset_t all;
        sigfillset(&all);
        for (int sig = 1; sig <= 64; sig++)
            if (sigismember(&all, sig) == 1 && sig != SIGKILL && sig != SIGSTOP)
                want |= 1ull << (sig - 1);
    }
    *count = 0;
    *blocking = 0;
    pid_t me = getpid();
    DIR *dir = opendir("/proc/self/task");
    if (!dir) abort();
    struct dirent *entry;
    while ((entry = readdir(dir))) {
        if (entry->d_name[0] == '.' || atoi(entry->d_name) == me) continue;
        char path[128], line[256];
        snprintf(path, sizeof path, "/proc/self/task/%s/status", entry->d_name);
        FILE *status = fopen(path, "r");
        if (!status) {
            if (!name) ++*count;
            continue;
        }
        if (name) {
            // The first line is `Name:\t<name>`.
            char want_line[64];
            snprintf(want_line, sizeof want_line, "Name:\t%s\n", name);
            if (!fgets(line, sizeof line, status) || strcmp(line, want_line) != 0) {
                fclose(status);
                continue;
            }
        }
        ++*count;
        while (fgets(line, sizeof line, status))
            // The mask in hex, highest signal first.
            if (strncmp(line, "SigBlk:", 7) == 0 && (strtoull(line + 7, NULL, 16) & want) == want)
                ++*blocking;
        fclose(status);
    }
    closedir(dir);
}

static void others(int *count, int *blocking) { others_named(NULL, count, blocking); }

// The allocator's thread names itself as it starts, so a look made just after it was created
// may come first: look again for up to a second while there is none.
static void allocator_threads(int *count, int *blocking) {
    for (int tries = 0; tries < 100; tries++) {
        others_named("cralloc", count, blocking);
        if (*count > 0) return;
        struct timespec t = {0, 10 * 1000000};
        nanosleep(&t, NULL);
    }
}

static void *nothing(void *arg) { return arg; }

static void one_thread(void) {
    pthread_t t;
    if (pthread_create(&t, NULL, nothing, NULL) != 0 || pthread_join(t, NULL) != 0) abort();
}

static int spawn_mode(void) {
    int count, blocking;
    one_thread();
    allocator_threads(&count, &blocking);
    printf("other threads %d, blocking every signal %d\n", count, blocking);
    fflush(stdout);
    pid_t child = fork();
    if (child == 0) {
        one_thread();
        allocator_threads(&count, &blocking);
        printf("child: other threads %d\n", count);
        exit(0);
    }
    int status = 0;
    if (waitpid(child, &status, 0) != child || !WIFEXITED(status) || WEXITSTATUS(status) != 0)
        return 1;
    return 0;
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

// Light work one object at a time, which never leaves the loaded magazines at all.
static void pairs(uint64_t *s) {
    static const size_t sizes[12] = {16, 24, 32, 40, 48, 64, 80, 96, 128, 160, 192, 256};
    for (int i = 0; i < 2000; i++) free(malloc(sizes[next(s) % 12]));
}

// A few slow paths: blocks too large for the magazines to hold forty of.
static void trickle(void) {
    void *held[40];
    for (int i = 0; i < 40; i++) {
        held[i] = malloc(8192);
        if (!held[i]) abort();
        memset(held[i], 2, 8192);
    }
    for (int i = 0; i < 40; i++) free(held[i]);
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

extern char **environ;

// As a program that sets its process title does (nginx, PostgreSQL): writes over the strings
// of its initial environment in place, then sets the variable another way, before its first
// thread.
static int rewrite_mode(void) {
    char *first = environ[0], *last = first;
    for (char **e = environ; *e; e++) last = *e + strlen(*e);
    if (first) memset(first, 'x', (size_t)(last - first));
    setenv("CRALLOC_BACKGROUND", "1", 1);
    int count, blocking;
    one_thread();
    allocator_threads(&count, &blocking);
    printf("other threads %d\n", count);
    return 0;
}

int main(int argc, char **argv) {
    if (argc > 1 && strcmp(argv[1], "threads") == 0) return threads_mode();
    if (argc > 1 && strcmp(argv[1], "rewrite") == 0) return rewrite_mode();
    if (argc > 1 && strcmp(argv[1], "spawn") == 0) return spawn_mode();
    if (argc > 2 && strcmp(argv[1], "silent") == 0) {
        one_thread();
        burst(1600000);
        sleep_ms(atol(argv[2]) * 1000);
        printf("end %ld\n", anon_kib() / 1024);
        return 0;
    }
    // The burst is freed before the program's first thread exists: the thread is created
    // dormant with the heap already over the floor, and must look at the level itself.
    if (argc > 2 && strcmp(argv[1], "late") == 0) {
        burst(1600000);
        one_thread();
        sleep_ms(atol(argv[2]) * 1000);
        printf("end %ld\n", anon_kib() / 1024);
        return 0;
    }
    if (argc > 2 && strcmp(argv[1], "exit") == 0) {
        atexit(at_exit);
        burst(200000);
        pthread_t t[4];
        for (long i = 0; i < 4; i++) pthread_create(&t[i], NULL, worker, (void *)(i + 1));
        sleep_ms(atol(argv[2]));
        puts("exit while maintaining");
        exit(0);
    }
    int trickling = argc > 2 && strcmp(argv[1], "trickle") == 0;
    int threaded = argc > 2 && strcmp(argv[1], "quiet") == 0;
    if (threaded) one_thread();
    if (argc > 2 && (trickling || threaded || strcmp(argv[1], "idle") == 0)) {
        long secs = atol(argv[2]);
        long rounds = 0;
        size_t n = 1600000;
        uint64_t s = 3;
        burst(n);
        long freed = anon_kib();
        struct timespec start, now;
        clock_gettime(CLOCK_MONOTONIC, &start);
        do {
            pairs(&s);
            // About every half second: each round sleeps a millisecond and works for a little.
            if (trickling && ++rounds % 450 == 0) trickle();
            sleep_ms(1);
            clock_gettime(CLOCK_MONOTONIC, &now);
        } while (now.tv_sec - start.tv_sec < secs);
        printf("freed %ld end %ld\n", freed / 1024, anon_kib() / 1024);
        return 0;
    }
    fprintf(stderr, "usage: background threads | spawn | exit MS | idle SECS | quiet SECS | silent SECS | "
                    "trickle SECS\n");
    return 2;
}
