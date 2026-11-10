// Threads created and destroyed in waves, blocks handed between threads and freed on another
// thread, every block's contents checked before it is freed, and an exit while other threads
// are still allocating. Prints only counts that do not depend on scheduling, so the output
// under any allocator is the same.
//
// usage: threads [waves threads ops]   or   threads exit
#include <malloc.h>
#include <pthread.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

#define SLOTS 1024

struct block {
    unsigned char *p;
    size_t size;
    uint64_t seed;
};

// One mailbox per thread: a thread posts blocks to its neighbour's, and frees what it finds in
// its own.
struct mailbox {
    pthread_mutex_t lock;
    struct block items[SLOTS];
    size_t count;
};

static struct mailbox *boxes;
static int nthreads;
static long ops_per_thread;
static long bad_blocks;
static pthread_mutex_t bad_lock = PTHREAD_MUTEX_INITIALIZER;

static uint64_t next(uint64_t *s) {
    *s ^= *s << 13;
    *s ^= *s >> 7;
    *s ^= *s << 17;
    return *s;
}

// A block's contents: one number of the seed's sequence for every eight bytes, and the low bytes
// of the next for what is left, so that its first n bytes depend only on the seed and n. A word at
// a time, since with blocks of up to a megabyte this is most of what the program does: another
// block's fill over any of these bytes shows in the word that holds them.
static void fill(struct block *b) {
    uint64_t s = b->seed | 1;
    size_t i = 0;
    for (; i + 8 <= b->size; i += 8) {
        uint64_t word = next(&s);
        memcpy(b->p + i, &word, 8);
    }
    if (i < b->size) {
        uint64_t word = next(&s);
        memcpy(b->p + i, &word, b->size - i);
    }
}

// Whether the first n bytes at p are what `fill` writes for this seed.
static int filled(const unsigned char *p, size_t n, uint64_t seed) {
    uint64_t s = seed | 1;
    size_t i = 0;
    for (; i + 8 <= n; i += 8) {
        uint64_t word = next(&s), found;
        memcpy(&found, p + i, 8);
        if (found != word) return 0;
    }
    if (i < n) {
        uint64_t word = next(&s);
        if (memcmp(p + i, &word, n - i) != 0) return 0;
    }
    return 1;
}

static int zeroed(const unsigned char *p, size_t n) {
    size_t i = 0;
    for (; i + 8 <= n; i += 8) {
        uint64_t found;
        memcpy(&found, p + i, 8);
        if (found != 0) return 0;
    }
    for (; i < n; i++)
        if (p[i] != 0) return 0;
    return 1;
}

static void check_and_free(struct block *b) {
    if (!filled(b->p, b->size, b->seed)) {
        pthread_mutex_lock(&bad_lock);
        bad_blocks++;
        pthread_mutex_unlock(&bad_lock);
    }
    free(b->p);
}

static size_t pick_size(uint64_t *s) {
    uint64_t r = next(s);
    switch (r % 16) {
    case 0: return (size_t)(r >> 8) % (1 << 20) + 1;  // up to a megabyte: large blocks
    case 1: case 2: return (size_t)(r >> 8) % 32768 + 1;
    default: return (size_t)(r >> 8) % 512;
    }
}

static void *worker(void *arg) {
    long me = (long)arg;
    uint64_t s = 0x9E3779B97F4A7C15ull * (uint64_t)(me + 1);
    struct block local[64];
    int held = 0;
    for (long op = 0; op < ops_per_thread; op++) {
        uint64_t r = next(&s);
        if (held < 64 && r % 3 != 0) {
            struct block b = {0};
            b.size = pick_size(&s);
            b.seed = next(&s);
            switch (r % 5) {
            case 0: b.p = calloc(1, b.size); break;
            case 1:
                if (posix_memalign((void **)&b.p, 64, b.size) != 0) b.p = NULL;
                if (b.p && ((uintptr_t)b.p & 63) != 0) abort();
                break;
            default: b.p = malloc(b.size); break;
            }
            if (!b.p && b.size) abort();
            if (r % 5 == 0 && !zeroed(b.p, b.size)) abort();
            fill(&b);
            local[held++] = b;
        } else if (held > 0) {
            struct block b = local[--held];
            if (r % 4 == 0) {
                // Grow or shrink it, check what was carried, and keep it.
                size_t to = pick_size(&s) + 1;
                unsigned char *q = realloc(b.p, to);
                if (!q) abort();
                if (!filled(q, b.size < to ? b.size : to, b.seed)) abort();
                b.p = q;
                b.size = to;
                fill(&b);
                local[held++] = b;
            } else if (r % 4 == 1) {
                struct mailbox *box = &boxes[(me + 1) % nthreads];
                pthread_mutex_lock(&box->lock);
                if (box->count < SLOTS) {
                    box->items[box->count++] = b;
                    b.p = NULL;
                }
                pthread_mutex_unlock(&box->lock);
                if (b.p) check_and_free(&b);
            } else {
                check_and_free(&b);
            }
        }
        if (op % 64 == 0) {
            struct mailbox *mine = &boxes[me];
            pthread_mutex_lock(&mine->lock);
            while (mine->count > 0) check_and_free(&mine->items[--mine->count]);
            pthread_mutex_unlock(&mine->lock);
        }
    }
    while (held > 0) check_and_free(&local[--held]);
    return NULL;
}

static volatile int stop;

// Allocation before main, and a block that lives until an exit handler frees it.
static char *from_constructor;

__attribute__((constructor(101))) static void early(void) {
    from_constructor = malloc(1000);
    memset(from_constructor, 'C', 1000);
}

static void at_exit(void) {
    for (int i = 0; i < 1000; i++)
        if (from_constructor[i] != 'C') abort();
    free(from_constructor);
    // Allocate during exit, while (in "exit" mode) other threads still run.
    for (int i = 0; i < 1000; i++) free(malloc((size_t)i * 64 + 1));
}

static void *forever(void *arg) {
    uint64_t s = (uint64_t)(uintptr_t)arg | 1;
    void *keep[32] = {0};
    while (!stop) {
        size_t i = next(&s) % 32;
        free(keep[i]);
        keep[i] = malloc(pick_size(&s) + 1);
    }
    return NULL;
}

int main(int argc, char **argv) {
    atexit(at_exit);
    if (argc > 1 && strcmp(argv[1], "exit") == 0) {
        // Exit, and run exit handlers and stdio teardown, while 16 threads allocate and free.
        pthread_t t[16];
        for (long i = 0; i < 16; i++) pthread_create(&t[i], NULL, forever, (void *)(i + 1));
        usleep(100 * 1000);
        puts("exit while allocating");
        exit(0);
    }
    int waves = argc > 1 ? atoi(argv[1]) : 20;
    nthreads = argc > 2 ? atoi(argv[2]) : 16;
    ops_per_thread = argc > 3 ? atol(argv[3]) : 20000;
    boxes = calloc((size_t)nthreads, sizeof *boxes);
    for (int i = 0; i < nthreads; i++) pthread_mutex_init(&boxes[i].lock, NULL);
    for (int w = 0; w < waves; w++) {
        pthread_t *t = malloc(sizeof(pthread_t) * (size_t)nthreads);
        for (long i = 0; i < nthreads; i++) pthread_create(&t[i], NULL, worker, (void *)i);
        for (int i = 0; i < nthreads; i++) pthread_join(t[i], NULL);
        free(t);
        // Whatever is still posted belongs to threads that have exited: free it here.
        for (int i = 0; i < nthreads; i++)
            while (boxes[i].count > 0) check_and_free(&boxes[i].items[--boxes[i].count]);
    }
    printf("waves %d threads %d ops %ld bad %ld\n", waves, nthreads, ops_per_thread, bad_blocks);
    free(boxes);
    return bad_blocks != 0;
}
