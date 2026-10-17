/* A preloadable stand-in allocator around the generated C-entry routines, so that a C program
 * can call our fast paths through the PLT next to the incumbents (cloop.c, compare.sh).
 *
 * Not a real allocator. The fast paths are the generated routines, with real restartable
 * sequences on glibc's rseq area. The slow paths below are plain C for a single-threaded
 * process: they edit the calling CPU's slab without a restartable sequence or an atomic, carve
 * objects from one region per class and never return memory. Region c holds class c, so the
 * region table is fixed; regions are 4 GiB of address space reserved without backing.
 */
#define _GNU_SOURCE
#include <errno.h>
#include <stdint.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/rseq.h>
#include <unistd.h>

#include "classes.h"

#define HIDDEN __attribute__((visibility("hidden")))
#define EXPORT __attribute__((visibility("default")))

void S_data(struct front_data *);
void *malloc(size_t);
void free(void *);
/* The same routines, called directly instead of through the PLT. */
HIDDEN void *S_malloc_fast(size_t);
HIDDEN void S_free_fast(void *);

/* CALLOC: 0 calls malloc and memset through the PLT; 1 calls the malloc routine directly and
 * zeroes in 16-byte stores; 2 the same with `rep stosb`; 3 calloc is a generated routine that
 * jumps to S_calloc_zero after its pop. */
#ifndef CALLOC
#define CALLOC 1
#endif

enum { SHIFT = 32 };
static struct front_data d;
static uintptr_t heap;
static char *carve[CLASS_COUNT + 1];   /* next never-used object of each class */
static void **spill[CLASS_COUNT + 1];  /* objects a full magazine gave up, and freed large objects */
static size_t nspill[CLASS_COUNT + 1];
static char *mag_next, *mag_end;       /* magazine memory */
enum { SPILL_CAP = 1 << 22 };

static void *reserve(size_t len) {
    void *p = mmap(NULL, len, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS | MAP_NORESERVE, -1, 0);
    return p == MAP_FAILED ? NULL : p;
}

static void die(const char *msg) {
    (void)!write(2, msg, strlen(msg));
    _exit(99);
}

static void init(void) {
    if (heap) return;
    S_data(&d);
    size_t span = (size_t)(CLASS_COUNT + 2) << SHIFT;
    char *raw = reserve(span);
    if (!raw) die("standin: cannot reserve the heap\n");
    uintptr_t base = ((uintptr_t)raw + ((uintptr_t)1 << SHIFT) - 1) & ~(((uintptr_t)1 << SHIFT) - 1);
    for (unsigned c = 1; c <= CLASS_COUNT; c++) {
        carve[c] = (char *)(base + ((uintptr_t)c << SHIFT));
        spill[c] = reserve(SPILL_CAP * sizeof(void *));
        if (!spill[c]) die("standin: cannot reserve spill stacks\n");
    }
    mag_next = reserve((size_t)1 << 30);
    if (!mag_next) die("standin: cannot reserve magazines\n");
    mag_end = mag_next + ((size_t)1 << 30);
    heap = base;
    setup_tables(&d, base, SHIFT);
}

static int cpu_now(void) {
    volatile struct rseq *rs = (struct rseq *)((char *)__builtin_thread_pointer() + __rseq_offset);
    int cpu = (int)rs->cpu_id;
    return cpu >= 0 && cpu < 8192 ? cpu : -1;
}

static uint64_t *slab_word(int cpu, unsigned c) { return (uint64_t *)(d.slabs + ((size_t)cpu << 12) + 8 * c); }

static void *carve_one(unsigned c) {
    if (nspill[c]) return spill[c][--nspill[c]];
    char *p = carve[c];
    if (class_size(c) > ((size_t)1 << SHIFT) || (uintptr_t)p + class_size(c) > heap + ((uintptr_t)(c + 1) << SHIFT))
        return NULL;
    carve[c] = p + class_size(c);
    return p;
}

/* The magazine word for class c on this CPU, with a magazine installed. */
static uint64_t *magazine(int cpu, unsigned c) {
    uint64_t *w = slab_word(cpu, c);
    if (*w == 0) {
        size_t bytes = 8 * (size_t)mag_slots(c);
        char *m = (char *)(((uintptr_t)mag_next + bytes - 1) & ~(uintptr_t)(bytes - 1));
        if (m + bytes > mag_end) die("standin: out of magazines\n");
        mag_next = m + bytes;
        *w = (uint64_t)(uintptr_t)(m + 8); /* slot 1: empty */
    }
    return w;
}

static void *small_alloc(unsigned c) {
    int cpu = cpu_now();
    if (cpu < 0) return carve_one(c);
    uint64_t *w = magazine(cpu, c);
    unsigned n = mag_slots(c);
    char *top = (char *)(uintptr_t)*w;
    /* Refill to half. */
    while (((uintptr_t)top & (8 * n - 1)) / 8 < n / 2) {
        void *p = carve_one(c);
        if (!p) break;
        *(void **)top = p;
        top += 8;
    }
    if (((uintptr_t)top & (8 * n - 1)) / 8 <= 1) {
        *w = (uint64_t)(uintptr_t)top;
        return carve_one(c);
    }
    top -= 8;
    void *p = *(void **)top;
    *w = (uint64_t)(uintptr_t)top;
    return p;
}

static void small_free(unsigned c, void *p) {
    int cpu = cpu_now();
    if (cpu < 0) {
        spill[c][nspill[c]++] = p;
        return;
    }
    uint64_t *w = magazine(cpu, c);
    unsigned n = mag_slots(c);
    char *top = (char *)(uintptr_t)*w;
    /* Spill down to half. */
    while (((uintptr_t)top & (8 * n - 1)) / 8 > n / 2) {
        top -= 8;
        if (nspill[c] == SPILL_CAP) die("standin: spill stack full\n");
        spill[c][nspill[c]++] = *(void **)top;
    }
    *(void **)top = p;
    *w = (uint64_t)(uintptr_t)(top + 8);
}

static unsigned class_of_ptr(const void *p) {
    uintptr_t a = (uintptr_t)p;
    if (!heap || a < heap || a - heap >= ((uintptr_t)(CLASS_COUNT + 1) << SHIFT)) return 0;
    return (unsigned)((a - heap) >> SHIFT);
}

static void *alloc_class(unsigned c) {
    if (c <= SMALL) return small_alloc(c);
    return carve_one(c);
}

/* The routines' slow paths. */
HIDDEN void *S_malloc_slow(size_t size) {
    init();
    if (size > ((size_t)1 << SHIFT)) {
        errno = ENOMEM;
        return NULL;
    }
    void *p = alloc_class(class_of_c(size));
    if (!p) errno = ENOMEM;
    return p;
}

HIDDEN void S_free_slow(void *p) {
    unsigned c = class_of_ptr(p);
    if (c == 0) return; /* NULL, or memory that was never ours */
    if (c <= SMALL)
        small_free(c, p);
    else
        spill[c][nspill[c]++] = p;
}

/* The generic routines' slow paths (the "rust" stand-in). Before the tables are set up the
 * size table reads class 0 and the size is gone; any size that reaches the generic pop is at
 * most 1024 bytes, so a 1024-byte object serves it. */
HIDDEN void *S_pop_slow(size_t c) {
    if (!heap || c == 0) {
        init();
        c = class_of_c(1024);
    }
    return alloc_class((unsigned)c);
}

HIDDEN void S_push_slow(size_t c, void *p) {
    (void)c;
    S_free_slow(p);
}

/* Zeroes the first n bytes of a block of class_of_c(n). Rounding up to 16 stays inside the
 * block: every C class above 8 bytes is a multiple of 16. */
static inline void zero_block(void *p, size_t n) {
#if CALLOC == 2
    size_t m = n <= 8 ? 8 : (n + 15) & ~(size_t)15;
    __asm__ volatile("rep stosb" : "+D"(p), "+c"(m) : "a"(0) : "memory");
#else
    if (n <= 8) {
        *(volatile uint64_t *)p = 0;
        return;
    }
    if (n > 512) {
        memset(p, 0, n);
        return;
    }
    typedef long long v2 __attribute__((vector_size(16)));
    char *q = p;
    for (size_t i = 0; i < n; i += 16) *(volatile v2 *)(q + i) = (v2){0, 0};
#endif
}

/* The generated calloc routines' tail and slow path (every build links them). */
HIDDEN void *S_calloc_zero(void *p, size_t n) {
    zero_block(p, n);
    return p;
}

HIDDEN void *S_calloc_slow(size_t total) {
    /* A product that overflowed arrives as SIZE_MAX, which no class holds. */
    void *p = S_malloc_slow(total);
    if (p) memset(p, 0, total);
    return p;
}

#if CALLOC != 3
EXPORT void *calloc(size_t n, size_t size) {
    size_t total;
    if (__builtin_mul_overflow(n, size, &total)) {
        errno = ENOMEM;
        return NULL;
    }
#if CALLOC == 0
    void *p = malloc(total);
    if (p) memset(p, 0, total);
#else
    void *p = S_malloc_fast(total);
    if (p) zero_block(p, total);
#endif
    return p;
}
#endif

EXPORT size_t malloc_usable_size(void *p) {
    unsigned c = class_of_ptr(p);
    return c ? class_size(c) : 0;
}

EXPORT void *realloc(void *p, size_t size) {
    if (!p) return S_malloc_fast(size);
    if (size == 0) {
        S_free_fast(p);
        return NULL;
    }
    unsigned c = class_of_ptr(p);
    if (c && class_of_c(size) == c) return p;
    void *q = S_malloc_fast(size);
    if (!q) return NULL;
    size_t old = c ? class_size(c) : 0;
    memcpy(q, p, old < size ? old : size);
    S_free_fast(p);
    return q;
}

/* Classes give alignment by construction: the class of a multiple of A is a multiple of A, and
 * a slot is aligned to the largest power of two dividing its class. */
static void *aligned(size_t align, size_t size) {
    if (align <= 16) return malloc(size);
    if (size == 0) size = 1;
    size_t rounded = (size + align - 1) & ~(align - 1);
    if (rounded < size) return NULL;
    return malloc(class_size(class_of_size(rounded)));
}

EXPORT int posix_memalign(void **out, size_t align, size_t size) {
    if (align < sizeof(void *) || (align & (align - 1))) return EINVAL;
    void *p = aligned(align, size);
    if (!p) return ENOMEM;
    *out = p;
    return 0;
}

EXPORT void *aligned_alloc(size_t align, size_t size) {
    if (align == 0 || (align & (align - 1))) {
        errno = EINVAL;
        return NULL;
    }
    return aligned(align, size);
}

EXPORT void *memalign(size_t align, size_t size) { return aligned_alloc(align, size); }
EXPORT void *valloc(size_t size) { return aligned(4096, size); }
EXPORT void *pvalloc(size_t size) { return aligned(4096, (size + 4095) & ~(size_t)4095); }
