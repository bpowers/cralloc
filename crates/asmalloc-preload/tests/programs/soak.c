// A stress harness for any malloc: workloads that are hard on a per-CPU allocator, with every
// block checked. Each block is filled with a pattern derived from a seed and its offset when it
// is allocated, and the pattern is verified before every free and realloc and on random looks
// in between, so silent corruption becomes a failure that names the block, the offset and the
// bytes found there. A table of live blocks catches an allocator that hands out a block that
// is already live, at the moment it does so.
//
// usage: soak MODE [key=value...]
//   modes:   churn, prodcons, fork, threadstorm, realloc, huge, signal, idle, breakpoint, trim,
//            sweep, quiet
//   keys:    threads, secs, seed, maxsize, live, xchg (percent of ops that hand a block to
//            another thread), sigus (signal interval, microseconds), usable (fill up to
//            malloc_usable_size, default 1), budget (MiB live across all threads, default
//            256; each thread may hold at least one block of maxsize), trimus (the trim
//            mode's interval, microseconds, default 2000)
//
// It prints one line that does not depend on scheduling ("soak MODE: ok"), so the output under
// any allocator is the same, and counts on stderr. A failure prints "soak: FAIL ..." on stderr
// and aborts. The harness keeps its own records in mmap'd memory, never on the heap it tests.
//
// The breakpoint mode aborts the allocator's restartable sequences at chosen instructions. It
// finds their descriptors in the `__rseq_cs` section of the shared object whose name contains
// "asmalloc", and every few operations arms a hardware execution breakpoint (perf, with
// SIGTRAP) on an instruction of a random sequence; the trap aborts the sequence there, and its
// handler allocates and frees before the sequence restarts. SOAK_INSNS names a file of the
// object's instruction addresses, one hex link-time address per line (from objdump); without
// it the breakpoint goes on a random byte, which fires only when that byte starts an
// instruction. Without such an object the mode does nothing.
//
// The sweep mode is the breakpoint mode made exhaustive and deterministic: one thread places
// the breakpoint on each listed instruction of each sequence in turn (SOAK_INSNS is required)
// and runs operations until it fires or `sweepops` operations pass, while `threads - 1` others
// churn without breakpoints. It reports how many sequences and instructions fired, and fails if
// a sequence never fired, unless SOAK_UNREACHED lists it, and if a listed one did fire, so the
// list cannot go stale. SOAK_FUNCS names the object's functions (`nm -C -S --defined-only`
// output), so that sequences in local functions get names; SOAK_UNREACHED has one line per
// sequence expected never to run: a substring of its function's name, optionally `#k` for the
// k-th sequence of that function in address order (from 0), `|`, and the reason.
//
// The signal and breakpoint modes allocate in signal handlers, which C does not allow and
// glibc's malloc does not support; they are for allocators that promise it (asmalloc's design:
// "Signals").
#define _GNU_SOURCE
#include <dlfcn.h>
#include <elf.h>
#include <fcntl.h>
#include <link.h>
#include <linux/hw_breakpoint.h>
#include <linux/perf_event.h>
#include <sys/ioctl.h>
#include <errno.h>
#include <malloc.h>
#include <pthread.h>
#include <sched.h>
#include <signal.h>
#include <stdarg.h>
#include <stdatomic.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/syscall.h>
#include <sys/time.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

// ---- configuration ----

static const char *mode;
static int nthreads = 8;
static double secs = 5;
static uint64_t seed0 = 1;
static size_t maxsize = 4 << 20;
static int live_cap = 2000;
static int xchg_pct = 5;
static long sig_us = 50;
static int use_usable = 1;
static size_t budget_mb = 256;

// ---- failure reports, without the heap ----

static pid_t tid(void) { return (pid_t)syscall(SYS_gettid); }

static void say(const char *fmt, ...) {
    char buf[2048];
    va_list ap;
    va_start(ap, fmt);
    int n = vsnprintf(buf, sizeof buf, fmt, ap);
    va_end(ap);
    if (n < 0) return;
    if ((size_t)n >= sizeof buf) n = sizeof buf - 1;
    ssize_t w = write(2, buf, (size_t)n);
    (void)w;
}

#define die(...)                                                                              \
    do {                                                                                      \
        say("soak: FAIL [%s pid %d tid %d] ", mode, getpid(), tid());                        \
        say(__VA_ARGS__);                                                                     \
        say("\n");                                                                            \
        abort();                                                                              \
    } while (0)

// ---- harness memory, outside the heap under test ----

static void *map(size_t n) {
    void *p = mmap(NULL, n, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS | MAP_NORESERVE,
                   -1, 0);
    if (p == MAP_FAILED) die("harness mmap of %zu bytes failed", n);
    return p;
}

static void unmap(void *p, size_t n) { munmap(p, n); }

// ---- randomness ----

static uint64_t mix64(uint64_t x) {
    x += 0x9e3779b97f4a7c15ull;
    x = (x ^ (x >> 30)) * 0xbf58476d1ce4e5b9ull;
    x = (x ^ (x >> 27)) * 0x94d049bb133111ebull;
    return x ^ (x >> 31);
}

typedef struct {
    uint64_t s;
} Rng;

static uint64_t rnd(Rng *r) { return mix64(r->s++); }
static uint64_t below(Rng *r, uint64_t n) { return n ? rnd(r) % n : 0; }

static double now(void) {
    struct timespec t;
    clock_gettime(CLOCK_MONOTONIC, &t);
    return (double)t.tv_sec + (double)t.tv_nsec * 1e-9;
}

static void sleep_us(long us) {
    struct timespec t = {us / 1000000, (us % 1000000) * 1000};
    while (nanosleep(&t, &t) == -1 && errno == EINTR) {
    }
}

// ---- patterns ----
//
// The byte at offset i of a block with seed s is byte i % 8 of mix64(s ^ i / 8). It depends on
// nothing else, so a realloc that keeps the seed keeps the pattern of the prefix it copies.
// Blocks above 1 MiB are covered sparsely: the first 64 bytes of every stride and the last 64
// bytes, so that a huge block is checked at every page (or every MiB above 64 MiB) without
// writing all of it.

#define DENSE_MAX (1u << 20)
#define STRIPE 64

typedef struct {
    unsigned char *p;
    size_t req;     // bytes asked for
    size_t n;       // bytes covered by the pattern (the usable size, or req)
    uint64_t seed;  // pattern seed, kept across reallocs
    uint64_t serial;
    int zeroed;     // from calloc: req bytes were checked zero, then filled
} Rec;

static size_t stride_of(size_t n) {
    if (n <= DENSE_MAX) return 0;
    return n > (64u << 20) ? (1u << 20) : 4096;
}

static inline unsigned char pat(uint64_t s, size_t i) {
    return (unsigned char)(mix64(s ^ (i >> 3)) >> ((i & 7) * 8));
}

static void fill_range(unsigned char *p, size_t a, size_t b, uint64_t s) {
    size_t i = a;
    for (; i < b && (i & 7); i++) p[i] = pat(s, i);
    for (; i + 8 <= b; i += 8) {
        uint64_t w = mix64(s ^ (i >> 3));
        memcpy(p + i, &w, 8);
    }
    for (; i < b; i++) p[i] = pat(s, i);
}

// The first offset in [a, b) that does not hold the pattern, or (size_t)-1.
static size_t check_range(const unsigned char *p, size_t a, size_t b, uint64_t s) {
    size_t i = a;
    for (; i < b && (i & 7); i++)
        if (p[i] != pat(s, i)) return i;
    for (; i + 8 <= b; i += 8) {
        uint64_t w = mix64(s ^ (i >> 3)), got;
        memcpy(&got, p + i, 8);
        if (got != w)
            for (size_t j = i; j < i + 8; j++)
                if (p[j] != pat(s, j)) return j;
    }
    for (; i < b; i++)
        if (p[i] != pat(s, i)) return i;
    return (size_t)-1;
}

static size_t min_sz(size_t a, size_t b) { return a < b ? a : b; }

// Visits the covered ranges of a pattern of n bytes that lie in [from, lim).
#define FOR_COVERED(n, from, lim, A, B, BODY)                                                 \
    do {                                                                                      \
        size_t st_ = stride_of(n), lim_ = min_sz((lim), (n)), from_ = (from);                 \
        if (st_ == 0) {                                                                       \
            size_t A = from_, B = lim_;                                                       \
            if (A < B) BODY;                                                                  \
        } else {                                                                              \
            for (size_t o_ = from_ - from_ % st_; o_ < lim_; o_ += st_) {                     \
                size_t A = o_ > from_ ? o_ : from_, B = min_sz(o_ + STRIPE, lim_);            \
                if (A < B) BODY;                                                              \
            }                                                                                 \
            size_t A = (n) - STRIPE > from_ ? (n) - STRIPE : from_, B = lim_;                 \
            if (A < B) BODY;                                                                  \
        }                                                                                     \
    } while (0)

// Fills the covered bytes from `from` on, and the last stripe of a sparse pattern whatever
// `from` is: a shrinking realloc can bring it into the kept prefix without its having been
// covered before.
static void fill(Rec *r, size_t from) {
    FOR_COVERED(r->n, from, r->n, a, b, fill_range(r->p, a, b, r->seed));
    if (stride_of(r->n)) fill_range(r->p, r->n - STRIPE, r->n, r->seed);
}

static void report(const Rec *r, size_t off, const char *when) {
    unsigned char *p = r->p;
    size_t lo = off >= 16 ? off - 16 : 0, hi = min_sz(off + 16, r->n);
    char hex[200];
    int k = 0;
    for (size_t i = lo; i < hi && k < 190; i++)
        k += snprintf(hex + k, sizeof hex - (size_t)k, "%s%02x", i == off ? "[" : "", p[i]);
    hex[k] = 0;
    uint64_t word = 0;
    size_t wo = off & ~(size_t)7;
    if (wo + 8 <= r->n) memcpy(&word, p + wo, 8);
    die("%s: block %p (req %zu, pattern %zu, serial %#lx) differs at offset %zu: "
        "found %02x, wanted %02x; word at +%zu is %#lx; bytes from +%zu: %s",
        when, (void *)p, r->req, r->n, (unsigned long)r->serial, off, p[off], pat(r->seed, off),
        wo, (unsigned long)word, lo, hex);
}

// Checks the covered bytes in [0, lim).
static void verify(const Rec *r, size_t lim, const char *when) {
    FOR_COVERED(r->n, 0, lim, a, b, {
        size_t bad = check_range(r->p, a, b, r->seed);
        if (bad != (size_t)-1) report(r, bad, when);
    });
}

// ---- the table of live blocks ----
//
// Open hashing into fixed buckets of seven words under a spin lock each: inserting a block
// that is already present means the allocator handed out a live block. A full bucket drops the
// block from the check (counted), which only weakens it. Off in forked children, whose locks
// another thread may have held at the fork, and in signal handlers.

#define LT_BUCKETS (1u << 20)
#define LT_WAYS 7

typedef struct {
    _Atomic uint32_t lock;
    uint32_t pad;
    uintptr_t slot[LT_WAYS];
} Bucket;

static Bucket *table;
static int table_on;
static _Atomic long table_dropped;

static Bucket *bucket(void *p) { return &table[mix64((uintptr_t)p) & (LT_BUCKETS - 1)]; }

static void lock_bucket(Bucket *b) {
    for (int spins = 0;; spins++) {
        uint32_t z = 0;
        if (atomic_compare_exchange_weak_explicit(&b->lock, &z, 1, memory_order_acquire,
                                                  memory_order_relaxed))
            return;
        if (spins > 64) sched_yield();
    }
}

static void unlock_bucket(Bucket *b) { atomic_store_explicit(&b->lock, 0, memory_order_release); }

static void live_insert(void *p, uint64_t serial) {
    if (!table_on || !p) return;
    Bucket *b = bucket(p);
    lock_bucket(b);
    int empty = -1;
    for (int i = 0; i < LT_WAYS; i++) {
        if (b->slot[i] == (uintptr_t)p) {
            unlock_bucket(b);
            die("the allocator returned %p (serial %#lx), which is already live", p,
                (unsigned long)serial);
        }
        if (!b->slot[i] && empty < 0) empty = i;
    }
    if (empty >= 0)
        b->slot[empty] = (uintptr_t)p;
    else
        atomic_fetch_add(&table_dropped, 1);
    unlock_bucket(b);
}

static void live_remove(void *p) {
    if (!table_on || !p) return;
    Bucket *b = bucket(p);
    lock_bucket(b);
    for (int i = 0; i < LT_WAYS; i++)
        if (b->slot[i] == (uintptr_t)p) {
            b->slot[i] = 0;
            break;
        }
    unlock_bucket(b);
}

// ---- allocating and freeing, through every entry point ----

static void (*free_sized_fn)(void *, size_t);
// Bytes the program holds (requested sizes), and their peak: the ground truth to read an
// allocator's peak resident memory against.
static _Atomic long live_bytes, live_peak;

static void held(long delta) {
    long now_ = atomic_fetch_add_explicit(&live_bytes, delta, memory_order_relaxed) + delta;
    long seen = atomic_load_explicit(&live_peak, memory_order_relaxed);
    while (now_ > seen && !atomic_compare_exchange_weak_explicit(&live_peak, &seen, now_,
                                                                 memory_order_relaxed,
                                                                 memory_order_relaxed)) {
    }
}
static _Atomic uint64_t serials;
static _Atomic long n_alloc, n_free, n_realloc, n_verify, n_xchg, n_fork, n_thread, n_signal;

static size_t draw_size(Rng *r) {
    uint64_t k = below(r, 1000);
    size_t lo, hi;
    if (k < 700)
        lo = 1, hi = 256;
    else if (k < 900)
        lo = 257, hi = 4096;
    else if (k < 980)
        lo = 4097, hi = 128 << 10;
    else if (k < 998)
        lo = (128 << 10) + 1, hi = 4 << 20;
    else
        lo = (4 << 20) + 1, hi = 64 << 20;
    if (hi > maxsize) hi = maxsize;
    if (lo > hi) lo = hi;
    // Within a band, every doubling equally likely.
    size_t s = lo + below(r, hi - lo + 1);
    if (below(r, 2)) {
        int bits = 1 + (int)below(r, 63 - __builtin_clzl(hi | 1));
        size_t t = (size_t)1 << bits;
        t += below(r, t);
        if (t >= lo && t <= hi) s = t;
    }
    if (below(r, 200) == 0) s = 0;
    return s;
}

static size_t draw_align(Rng *r) {
    uint64_t k = below(r, 1000);
    if (k < 990) return (size_t)8 << below(r, 10);  // 8 to 4096
    if (k < 998) return (size_t)8192 << below(r, 4);
    return (size_t)2 << 20;
}

static void check_align(void *p, size_t req, size_t align, const char *what) {
    uintptr_t a = (uintptr_t)p;
    size_t want = align;
    if (want < 16 && req >= 16) want = 16;
    if (want < 8) want = 8;
    if (a & (want - 1)) die("%s of %zu (align %zu) returned %p, not %zu-aligned", what, req,
                            align, p, want);
}

// Allocates about `req` bytes through a random entry point and fills it. 0 if the allocator
// returned null, which is acceptable only for a zero size or under memory pressure (counted).
static _Atomic long n_null;

static int alloc_rec(Rng *r, size_t req, Rec *out) {
    void *p = NULL;
    size_t align = 0;
    int zeroed = 0;
    uint64_t k = below(r, 100);
    const char *what = "malloc";
    if (k < 60) {
        p = malloc(req);
    } else if (k < 70) {
        size_t n = 1 + below(r, 16), m = req / n;
        req = n * m;
        p = below(r, 2) ? calloc(n, m) : calloc(m, n);
        zeroed = 1;
        what = "calloc";
    } else if (k < 76) {
        align = draw_align(r);
        if (posix_memalign(&p, align, req) != 0) p = NULL;
        what = "posix_memalign";
    } else if (k < 81) {
        align = draw_align(r);
        req = (req + align - 1) / align * align;
        p = aligned_alloc(align, req);
        what = "aligned_alloc";
    } else if (k < 84) {
        align = draw_align(r);
        p = memalign(align, req);
        what = "memalign";
    } else if (k < 86) {
        align = 4096;
        p = valloc(req);
        what = "valloc";
    } else if (k < 87) {
        align = 4096;
        p = pvalloc(req);
        req = (req + 4095) & ~(size_t)4095;
        what = "pvalloc";
    } else if (k < 92) {
        p = realloc(NULL, req);
        what = "realloc(NULL)";
    } else if (k < 95) {
        size_t n = 1 + below(r, 8), m = req / n;
        req = n * m;
        p = reallocarray(NULL, n, m);
        what = "reallocarray(NULL)";
    } else {
        p = malloc(req);
    }
    if (!p) {
        if (req) atomic_fetch_add(&n_null, 1);
        return 0;
    }
    check_align(p, req, align, what);
    uint64_t serial = atomic_fetch_add(&serials, 1);
    live_insert(p, serial);
    size_t n = use_usable ? malloc_usable_size(p) : req;
    if (n < req) die("%s(%zu) returned %p with usable size %zu", what, req, p, n);
    Rec rec = {p, req, n, mix64(serial ^ seed0), serial, zeroed};
    if (zeroed)
        FOR_COVERED(req, 0, req, a, b, {
            for (size_t i = a; i < b; i++)
                if (rec.p[i])
                    die("calloc of %zu returned %p with byte %zu = %02x", req, p, i, rec.p[i]);
        });
    fill(&rec, 0);
    *out = rec;
    held((long)req);
    atomic_fetch_add(&n_alloc, 1);
    return 1;
}

static void free_rec(Rng *r, Rec *b) {
    verify(b, b->n, "before free");
    live_remove(b->p);
    uint64_t k = below(r, 100);
    if (k < 8 && free_sized_fn)
        free_sized_fn(b->p, b->req);
    else if (k < 11) {
        void *gone = realloc(b->p, 0);  // frees and returns null, as in glibc
        if (gone) die("realloc(%p, 0) returned %p", (void *)b->p, gone);
    }
    else
        free(b->p);
    b->p = NULL;
    held(-(long)b->req);
    atomic_fetch_add(&n_free, 1);
}

// Resizes the block, mostly by growth as vectors grow, and checks the prefix it keeps.
static void realloc_rec(Rec *b, size_t new_req) {
    verify(b, b->n, "before realloc");
    live_remove(b->p);
    void *q = realloc(b->p, new_req);
    if (!q) {
        live_insert(b->p, b->serial);
        verify(b, b->n, "after a failed realloc");
        atomic_fetch_add(&n_null, 1);
        return;
    }
    check_align(q, new_req, 0, "realloc");
    live_insert(q, b->serial);
    // With `usable`, the program wrote all of the usable size, and realloc must keep that much
    // (glibc's does, and programs such as redis rely on it), not just the size asked for.
    size_t keep = min_sz(use_usable ? b->n : b->req, new_req);
    Rec old = *b;
    old.p = q;
    verify(&old, keep, "after realloc (the prefix it must keep)");
    size_t n = use_usable ? malloc_usable_size(q) : new_req;
    if (n < new_req) die("realloc(%zu) returned %p with usable size %zu", new_req, q, n);
    held((long)new_req - (long)b->req);
    b->p = q;
    b->req = new_req;
    b->n = n;
    // A denser covering than the old one has holes in the prefix; refill all of it.
    fill(b, stride_of(n) < stride_of(old.n) ? 0 : keep);
    atomic_fetch_add(&n_realloc, 1);
}

static size_t regrow(Rng *r, size_t old) {
    uint64_t k = below(r, 10);
    size_t s;
    if (k < 6)
        s = old + old * (25 + below(r, 76)) / 100 + 1;
    else if (k < 8)
        s = old / (1 + below(r, 4));
    else
        s = draw_size(r);
    if (s > maxsize) s = maxsize;
    if (s == 0) s = 1;
    return s;
}

// ---- per-thread sets of blocks ----

typedef struct {
    Rec *recs;
    int count, cap;
    size_t bytes, budget;
} Set;

static void set_init(Set *s, int cap, size_t budget) {
    s->recs = map(sizeof(Rec) * (size_t)cap);
    s->count = 0;
    s->cap = cap;
    s->bytes = 0;
    s->budget = budget;
}

static void set_add(Set *s, Rec r) {
    s->recs[s->count++] = r;
    s->bytes += r.n;
}

static Rec set_take(Set *s, int i) {
    Rec r = s->recs[i];
    s->recs[i] = s->recs[--s->count];
    s->bytes -= r.n;
    return r;
}

static void set_free_all(Rng *r, Set *s) {
    while (s->count) {
        Rec b = set_take(s, s->count - 1);
        free_rec(r, &b);
    }
}

static void set_done(Set *s) { unmap(s->recs, sizeof(Rec) * (size_t)s->cap); }

// ---- hand-off between threads ----

#define XSLOTS 4096

typedef struct {
    _Atomic uint32_t lock;
    int full;
    Rec r;
} XSlot;

static XSlot *xs;

static void xlock(XSlot *x) { lock_bucket((Bucket *)x); }
static void xunlock(XSlot *x) { unlock_bucket((Bucket *)x); }

// Puts a block in a random slot and takes whatever was there.
static int xchg(Rng *r, Rec *give, Rec *got) {
    XSlot *x = &xs[below(r, XSLOTS)];
    xlock(x);
    int had = x->full;
    if (had) *got = x->r;
    x->r = *give;
    x->full = 1;
    xunlock(x);
    atomic_fetch_add(&n_xchg, 1);
    return had;
}

static int xput(Rng *r, Rec *give) {
    for (int tries = 0; tries < 64; tries++) {
        XSlot *x = &xs[below(r, XSLOTS)];
        xlock(x);
        if (!x->full) {
            x->r = *give;
            x->full = 1;
            xunlock(x);
            return 1;
        }
        xunlock(x);
    }
    return 0;
}

static int xget(Rng *r, Rec *got) {
    for (int tries = 0; tries < 64; tries++) {
        XSlot *x = &xs[below(r, XSLOTS)];
        xlock(x);
        if (x->full) {
            *got = x->r;
            x->full = 0;
            xunlock(x);
            return 1;
        }
        xunlock(x);
    }
    return 0;
}

static void xdrain(Rng *r) {
    for (int i = 0; i < XSLOTS; i++)
        if (xs[i].full) {
            free_rec(r, &xs[i].r);
            xs[i].full = 0;
        }
}

// ---- one random operation on a set ----

static void step(Rng *r, Set *s, int xpct) {
    uint64_t k = below(r, 100);
    if (k < (uint64_t)xpct && s->count && xs) {
        int i = (int)below(r, (uint64_t)s->count);
        Rec give = set_take(s, i), got;
        if (xchg(r, &give, &got)) {
            verify(&got, got.n, "received from another thread");
            if (below(r, 2))
                free_rec(r, &got);
            else
                set_add(s, got);
        }
        return;
    }
    int want_alloc = s->count < s->cap / 2 || (s->count < s->cap && below(r, 2));
    if (k < 50 && want_alloc) {
        size_t req = draw_size(r);
        if (s->bytes + req > s->budget) req = draw_size(r) % 4096;
        Rec b;
        if (s->bytes + req <= s->budget && alloc_rec(r, req, &b)) set_add(s, b);
        return;
    }
    if (!s->count) return;
    int i = (int)below(r, (uint64_t)s->count);
    if (k < 85) {
        Rec b = set_take(s, i);
        free_rec(r, &b);
    } else if (k < 95) {
        Rec b = set_take(s, i);
        size_t to = regrow(r, b.req);
        if (s->bytes + to > s->budget) to = b.req / 2 + 1;
        realloc_rec(&b, to);
        set_add(s, b);
    } else {
        verify(&s->recs[i], s->recs[i].n, "on a look");
        atomic_fetch_add(&n_verify, 1);
    }
}

// ---- modes ----

static double deadline;
static _Atomic int stop;

static int done(void) {
    return atomic_load_explicit(&stop, memory_order_relaxed) || now() > deadline;
}

static size_t per_thread_budget(void) {
    size_t b = (budget_mb << 20) / (size_t)nthreads;
    return b < maxsize ? maxsize : b;
}

static void *churn_worker(void *arg) {
    Rng r = {mix64(seed0 ^ (uintptr_t)arg)};
    Set s;
    set_init(&s, live_cap, per_thread_budget());
    long ops = 0;
    while (!(ops++ % 64 == 0 && done())) step(&r, &s, xchg_pct);
    set_free_all(&r, &s);
    set_done(&s);
    return NULL;
}

static void run_threads(void *(*fn)(void *), int n) {
    pthread_t *t = map(sizeof(pthread_t) * (size_t)n);
    for (long i = 0; i < n; i++)
        if (pthread_create(&t[i], NULL, fn, (void *)(i + 1)))
            die("pthread_create failed");
    for (int i = 0; i < n; i++) pthread_join(t[i], NULL);
    unmap(t, sizeof(pthread_t) * (size_t)n);
}

// trim: workers churn while the main thread calls malloc_trim every `trimus` microseconds, so
// an allocator that empties caches and returns memory there does it under live traffic.
static long trim_us = 2000;
static _Atomic long n_trim;

static void run_trim(void) {
    pthread_t *t = map(sizeof(pthread_t) * (size_t)nthreads);
    for (long i = 0; i < nthreads; i++)
        if (pthread_create(&t[i], NULL, churn_worker, (void *)(i + 1)))
            die("pthread_create failed");
    while (!done()) {
        malloc_trim(0);
        atomic_fetch_add(&n_trim, 1);
        sleep_us(trim_us);
    }
    for (int i = 0; i < nthreads; i++) pthread_join(t[i], NULL);
    unmap(t, sizeof(pthread_t) * (size_t)nthreads);
}

// quiet: workers churn for `secs`, free everything and exit; then the process makes no calls
// for `quietms` milliseconds while resident memory is sampled. Memory is counted above what
// was resident before the burst, and the live-block table is off so that the harness's own
// memory stays out of it. With SOAK_QUIET_CHECK set it fails unless what the allocator held
// just after the frees fell to `quietpct` percent, or 16 MiB: for an allocator that returns
// memory without being called (asmalloc with ASMALLOC_BACKGROUND=1).
static long quiet_ms = 3000, quiet_pct = 50;

static long rss_mib(void) {
    long pages = 0, rss = 0;
    FILE *f = fopen("/proc/self/statm", "r");
    if (f && fscanf(f, "%ld %ld", &pages, &rss) != 2) rss = 0;
    if (f) fclose(f);
    return rss * 4096 / (1 << 20);
}

static void run_quiet(void) {
    table_on = 0;
    long base = rss_mib();
    run_threads(churn_worker, nthreads);
    xdrain(&(Rng){1});
    long after = rss_mib() - base, low = after;
    for (long t = 0; t < quiet_ms; t += 100) {
        sleep_us(100000);
        long now_ = rss_mib() - base;
        if (now_ < low) low = now_;
    }
    long end = rss_mib() - base;
    say("soak quiet: %ld MiB resident before the burst; above that, %ld MiB after the frees and "
        "%ld after %ld ms quiet (lowest %ld)\n",
        base, after, end, quiet_ms, low);
    if (getenv("SOAK_QUIET_CHECK") && end > 16 && end * 100 > after * quiet_pct)
        die("resident memory stayed at %ld MiB of %ld after %ld ms quiet", end, after, quiet_ms);
}

// Producers allocate and only hand off; consumers only verify and free.
static _Atomic int producers_left;

static void *producer(void *arg) {
    Rng r = {mix64(seed0 ^ (uintptr_t)arg ^ 0x1111)};
    while (!done()) {
        Rec b;
        if (!alloc_rec(&r, draw_size(&r) % 8192, &b)) continue;
        while (!xput(&r, &b)) {
            if (done()) {
                free_rec(&r, &b);
                goto out;
            }
            sched_yield();
        }
    }
out:
    atomic_fetch_sub(&producers_left, 1);
    return NULL;
}

static void *consumer(void *arg) {
    Rng r = {mix64(seed0 ^ (uintptr_t)arg ^ 0x2222)};
    for (;;) {
        Rec b;
        if (xget(&r, &b)) {
            verify(&b, b.n, "received by a consumer");
            free_rec(&r, &b);
        } else if (atomic_load(&producers_left) == 0) {
            break;
        } else {
            sched_yield();
        }
    }
    return NULL;
}

static void *prodcons_worker(void *arg) {
    return ((uintptr_t)arg % 2) ? producer(arg) : consumer(arg);
}

// fork: workers churn while other threads fork. Each child checks and frees the blocks the
// forking thread held, allocates on two threads of its own, forks once more sometimes, and
// exits; the parent checks the exit status.
static void child_work(Rng *r, Set *inherited, int depth) {
    table_on = 0;
    xs = NULL;
    for (int i = 0; i < inherited->count; i++)
        verify(&inherited->recs[i], inherited->recs[i].n, "inherited by a forked child");
    while (inherited->count > inherited->cap / 4) {
        Rec b = set_take(inherited, (int)below(r, (uint64_t)inherited->count));
        free_rec(r, &b);
    }
    pthread_t t[2];
    for (long i = 0; i < 2; i++) {
        stop = 0;
        deadline = now() + 0.02;
        if (pthread_create(&t[i], NULL, churn_worker, (void *)(i + 100 + depth * 10)))
            die("pthread_create in a child failed");
    }
    for (int i = 0; i < 300; i++) step(r, inherited, 0);
    for (int i = 0; i < 2; i++) pthread_join(t[i], NULL);
    if (depth == 0 && below(r, 4) == 0) {
        pid_t pid = fork();
        if (pid < 0) die("fork in a child failed");
        if (pid == 0) {
            child_work(r, inherited, 1);
            _exit(0);
        }
        int st;
        if (waitpid(pid, &st, 0) != pid || !WIFEXITED(st) || WEXITSTATUS(st))
            die("a grandchild failed: status %#x", st);
    }
    set_free_all(r, inherited);
}

static void *forker(void *arg) {
    Rng r = {mix64(seed0 ^ (uintptr_t)arg ^ 0x3333)};
    Set s;
    set_init(&s, 256, per_thread_budget());
    while (!done()) {
        for (int i = 0; i < 200; i++) step(&r, &s, 0);
        pid_t pid = fork();
        if (pid < 0) die("fork failed: %d", errno);
        if (pid == 0) {
            child_work(&r, &s, 0);
            _exit(0);
        }
        int st;
        if (waitpid(pid, &st, 0) != pid) die("waitpid failed");
        if (!WIFEXITED(st) || WEXITSTATUS(st))
            die("a forked child failed: %s %d", WIFSIGNALED(st) ? "signal" : "exit status",
                WIFSIGNALED(st) ? WTERMSIG(st) : WEXITSTATUS(st));
        atomic_fetch_add(&n_fork, 1);
    }
    set_free_all(&r, &s);
    set_done(&s);
    return NULL;
}

static void *fork_worker(void *arg) {
    return ((uintptr_t)arg % 4 == 1) ? forker(arg) : churn_worker(arg);
}

// threadstorm: thousands of short threads, at most `threads` at once, each allocating a few
// blocks, trading some with threads that have already exited, and leaving.
static void *short_thread(void *arg) {
    Rng r = {mix64(seed0 ^ (uintptr_t)arg ^ 0x4444)};
    Rec b[16];
    int n = 1 + (int)below(&r, 16);
    for (int i = 0; i < n; i++)
        if (!alloc_rec(&r, draw_size(&r) % 16384, &b[i])) b[i].p = NULL;
    for (int i = 0; i < n; i++) {
        if (!b[i].p) continue;
        Rec got;
        if (!below(&r, 2)) {
            free_rec(&r, &b[i]);
        } else if (xchg(&r, &b[i], &got)) {
            verify(&got, got.n, "left by an exited thread");
            free_rec(&r, &got);
        }
    }
    atomic_fetch_add(&n_thread, 1);
    return NULL;
}

static void run_threadstorm(void) {
    pthread_t *t = map(sizeof(pthread_t) * (size_t)nthreads);
    uintptr_t id = 1;
    while (!done()) {
        for (int i = 0; i < nthreads; i++)
            if (pthread_create(&t[i], NULL, short_thread, (void *)id++))
                die("pthread_create failed");
        for (int i = 0; i < nthreads; i++) pthread_join(t[i], NULL);
    }
    unmap(t, sizeof(pthread_t) * (size_t)nthreads);
}

// realloc: growth chains up to maxsize and back down, contents checked at every step.
static void *realloc_worker(void *arg) {
    Rng r = {mix64(seed0 ^ (uintptr_t)arg ^ 0x5555)};
    while (!done()) {
        Rec b;
        if (!alloc_rec(&r, 1 + below(&r, 64), &b)) continue;
        size_t top = 1 + below(&r, maxsize);
        while (b.req < top && !done()) {
            size_t to = b.req + b.req * (5 + below(&r, 96)) / 100 + 1 + below(&r, 16);
            realloc_rec(&b, to > maxsize ? maxsize : to);
            if (b.req >= maxsize) break;
        }
        while (b.req > 1 && !done()) realloc_rec(&b, b.req / (2 + below(&r, 3)) + 1);
        free_rec(&r, &b);
    }
    return NULL;
}

// huge: blocks far above every class, alone and among small ones.
static void *huge_worker(void *arg) {
    Rng r = {mix64(seed0 ^ (uintptr_t)arg ^ 0x6666)};
    Set s;
    set_init(&s, 64, per_thread_budget());
    while (!done()) {
        if (s.count < 4) {
            size_t req = (maxsize / 4) + below(&r, maxsize - maxsize / 4 + 1);
            if (s.bytes + req <= s.budget) {
                Rec b;
                if (alloc_rec(&r, req, &b)) set_add(&s, b);
            }
        } else {
            int i = (int)below(&r, (uint64_t)s.count);
            Rec b = set_take(&s, i);
            if (below(&r, 3) == 0) {
                realloc_rec(&b, below(&r, 2) ? b.req / 2 + 1 : min_sz(b.req * 2, maxsize));
                if (s.bytes + b.n <= s.budget) {
                    set_add(&s, b);
                    continue;
                }
            }
            free_rec(&r, &b);
        }
        for (int i = 0; i < 20; i++) {
            Rec b;
            if (alloc_rec(&r, 1 + below(&r, 512), &b)) free_rec(&r, &b);
        }
    }
    set_free_all(&r, &s);
    set_done(&s);
    return NULL;
}

// signal: every worker has a timer that interrupts it every `sigus` microseconds of its CPU
// time, and the process has another on the wall clock; both handlers allocate and free.
// Handlers keep their blocks in rings of their own, one per signal, so a nested handler never
// shares state with the one it interrupted.
#define RING 32
static __thread Rec ring_thread[RING], ring_process[RING];
static __thread Rng sig_rng;
static __thread int sig_ready;

static void sig_body(Rec *ring) {
    int saved = errno;
    if (sig_ready) {
        Rec *slot = &ring[below(&sig_rng, RING)];
        if (slot->p) {
            if (below(&sig_rng, 4) == 0) {
                realloc_rec(slot, 1 + below(&sig_rng, 2048));
            } else {
                free_rec(&sig_rng, slot);
            }
        }
        if (!slot->p) {
            size_t req = below(&sig_rng, 16) ? below(&sig_rng, 512) : below(&sig_rng, 300000);
            if (!alloc_rec(&sig_rng, req, slot)) slot->p = NULL;
        }
        atomic_fetch_add_explicit(&n_signal, 1, memory_order_relaxed);
    }
    errno = saved;
}

static void on_thread_timer(int sig) {
    (void)sig;
    sig_body(ring_thread);
}

static void on_process_timer(int sig) {
    (void)sig;
    sig_body(ring_process);
}

static void *signal_worker(void *arg) {
    sig_rng.s = mix64(seed0 ^ (uintptr_t)arg ^ 0x7777);
    Rng r = {mix64(seed0 ^ (uintptr_t)arg ^ 0x8888)};
    sig_ready = 1;
    timer_t timer;
    struct sigevent ev;
    memset(&ev, 0, sizeof ev);
    ev.sigev_notify = SIGEV_THREAD_ID;
    ev.sigev_signo = SIGRTMIN;
    ev._sigev_un._tid = tid();
    if (timer_create(CLOCK_THREAD_CPUTIME_ID, &ev, &timer)) die("timer_create failed");
    struct itimerspec its = {{0, sig_us * 1000}, {0, sig_us * 1000}};
    timer_settime(timer, 0, &its, NULL);
    Set s;
    set_init(&s, live_cap, per_thread_budget());
    long ops = 0;
    while (!(ops++ % 64 == 0 && done())) step(&r, &s, xchg_pct);
    timer_delete(timer);
    sigset_t all;
    sigfillset(&all);
    pthread_sigmask(SIG_BLOCK, &all, NULL);
    sig_ready = 0;
    for (int i = 0; i < RING; i++) {
        if (ring_thread[i].p) free_rec(&r, &ring_thread[i]);
        if (ring_process[i].p) free_rec(&r, &ring_process[i]);
    }
    set_free_all(&r, &s);
    set_done(&s);
    return NULL;
}

static void run_signal(void) {
    table_on = 0;  // a handler could interrupt its own thread holding a bucket lock
    struct sigaction sa;
    memset(&sa, 0, sizeof sa);
    sa.sa_handler = on_thread_timer;
    sa.sa_flags = SA_RESTART;
    sigaction(SIGRTMIN, &sa, NULL);
    sa.sa_handler = on_process_timer;
    sigaction(SIGALRM, &sa, NULL);
    // The main thread takes no signals: its ring would race with the workers' teardown.
    sigset_t set;
    sigemptyset(&set);
    sigaddset(&set, SIGALRM);
    sigaddset(&set, SIGRTMIN);
    struct itimerval it = {{0, sig_us * 3}, {0, sig_us * 3}};
    pthread_t *t = map(sizeof(pthread_t) * (size_t)nthreads);
    for (long i = 0; i < nthreads; i++)
        if (pthread_create(&t[i], NULL, signal_worker, (void *)(i + 1)))
            die("pthread_create failed");
    pthread_sigmask(SIG_BLOCK, &set, NULL);
    setitimer(ITIMER_REAL, &it, NULL);
    for (int i = 0; i < nthreads; i++) pthread_join(t[i], NULL);
    struct itimerval off = {{0, 0}, {0, 0}};
    setitimer(ITIMER_REAL, &off, NULL);
    unmap(t, sizeof(pthread_t) * (size_t)nthreads);
}

// idle: threads that hop between CPUs, work in bursts, and go quiet for around a second, so
// that the CPUs they leave sit idle long enough for an allocator to drain their caches while
// they hold blocks from them, and come back while a drain may be running.
static cpu_set_t allowed;
static int allowed_cpus[CPU_SETSIZE], n_allowed;

static void *idle_worker(void *arg) {
    Rng r = {mix64(seed0 ^ (uintptr_t)arg ^ 0x9999)};
    Set s;
    set_init(&s, live_cap, per_thread_budget());
    int busy = (uintptr_t)arg % 4 == 0;  // a quarter never sleep and keep the slow paths going
    while (!done()) {
        cpu_set_t one;
        CPU_ZERO(&one);
        CPU_SET(allowed_cpus[below(&r, (uint64_t)n_allowed)], &one);
        sched_setaffinity(0, sizeof one, &one);
        int burst = 100 + (int)below(&r, 5000);
        for (int i = 0; i < burst; i++) step(&r, &s, xchg_pct);
        if (!busy && below(&r, 3) == 0) sleep_us(700000 + (long)below(&r, 900000));
    }
    sched_setaffinity(0, sizeof allowed, &allowed);
    set_free_all(&r, &s);
    set_done(&s);
    return NULL;
}

// trap: hardware breakpoints inside the allocator's restartable sequences.
typedef struct {
    uint32_t version, flags;
    uint64_t start_ip, post_commit_offset, abort_ip;
} RseqCs;

static RseqCs *sequences;
static size_t n_sequences;
static uintptr_t object_base;
static _Atomic long n_trap;
// Run-time addresses of the instructions inside the sequences, if SOAK_INSNS gave them.
static uint64_t *insns;
static size_t n_insns;

static int in_sequence(uint64_t a) {
    for (size_t i = 0; i < n_sequences; i++)
        if (a - sequences[i].start_ip < sequences[i].post_commit_offset) return 1;
    return 0;
}

static void load_insns(uintptr_t bias) {
    const char *path = getenv("SOAK_INSNS");
    if (!path) return;
    FILE *f = fopen(path, "r");
    if (!f) die("cannot open SOAK_INSNS file %s", path);
    size_t cap = 1 << 16;
    insns = map(cap * sizeof *insns);
    unsigned long long a;
    while (n_insns < cap && fscanf(f, "%llx", &a) == 1)
        if (in_sequence(bias + a)) insns[n_insns++] = bias + a;
    fclose(f);
}

static int find_sequences(struct dl_phdr_info *info, size_t size, void *data) {
    (void)size;
    (void)data;
    if (!info->dlpi_name || !strstr(info->dlpi_name, "asmalloc")) return 0;
    int fd = open(info->dlpi_name, O_RDONLY);
    if (fd < 0) return 0;
    Elf64_Ehdr eh;
    if (pread(fd, &eh, sizeof eh, 0) != sizeof eh || eh.e_shentsize != sizeof(Elf64_Shdr)) {
        close(fd);
        return 0;
    }
    size_t shn = eh.e_shnum;
    Elf64_Shdr *sh = map(sizeof(Elf64_Shdr) * shn);
    if (pread(fd, sh, sizeof(Elf64_Shdr) * shn, (off_t)eh.e_shoff) !=
        (ssize_t)(sizeof(Elf64_Shdr) * shn))
        die("reading the section headers of %s", info->dlpi_name);
    Elf64_Shdr *strs = &sh[eh.e_shstrndx];
    char *names = map(strs->sh_size);
    if (pread(fd, names, strs->sh_size, (off_t)strs->sh_offset) != (ssize_t)strs->sh_size)
        die("reading the section names of %s", info->dlpi_name);
    for (size_t i = 0; i < shn; i++)
        if (sh[i].sh_name < strs->sh_size && !strcmp(names + sh[i].sh_name, "__rseq_cs")) {
            // Mapped and relocated: the descriptors hold run-time addresses.
            sequences = (RseqCs *)(info->dlpi_addr + sh[i].sh_addr);
            n_sequences = sh[i].sh_size / sizeof(RseqCs);
            object_base = info->dlpi_addr;
        }
    if (n_sequences) load_insns(info->dlpi_addr);
    close(fd);
    return n_sequences != 0;
}

static __thread int trap_fd = -1;
static __thread Rec ring_trap[RING];
static __thread long trap_hits;

static void on_trap(int sig) {
    (void)sig;
    int saved = errno;
    if (trap_fd >= 0) ioctl(trap_fd, PERF_EVENT_IOC_DISABLE, 0);
    atomic_fetch_add_explicit(&n_trap, 1, memory_order_relaxed);
    trap_hits++;
    errno = saved;
    // The handler allocates, so the interrupted sequence's restart finds a slab that moved.
    sig_body(ring_trap);
}

static void disarm(void) {
    if (trap_fd >= 0) close(trap_fd);
    trap_fd = -1;
}

// Puts this thread's breakpoint on the instruction at `at`.
static void arm_at(uint64_t at) {
    disarm();
    struct perf_event_attr a;
    memset(&a, 0, sizeof a);
    a.type = PERF_TYPE_BREAKPOINT;
    a.size = sizeof a;
    a.sample_period = 1;
    a.exclude_kernel = 1;
    a.exclude_hv = 1;
    a.remove_on_exec = 1;
    a.sigtrap = 1;
    a.bp_type = HW_BREAKPOINT_X;
    a.bp_addr = at;
    a.bp_len = sizeof(long);
    trap_fd = (int)syscall(SYS_perf_event_open, &a, 0, -1, -1, 0);
    if (trap_fd < 0) die("perf_event_open for a breakpoint failed: %d", errno);
}

// Moves this thread's breakpoint to a random instruction of a random sequence, or without a
// list to a random byte, which fires only when it starts an instruction.
static void arm(Rng *r) {
    if (n_insns) {
        arm_at(insns[below(r, n_insns)]);
        return;
    }
    const RseqCs *cs = &sequences[below(r, n_sequences)];
    if (cs->post_commit_offset) arm_at(cs->start_ip + below(r, cs->post_commit_offset));
}

static void *trap_worker(void *arg) {
    sig_rng.s = mix64(seed0 ^ (uintptr_t)arg ^ 0xaaaa);
    Rng r = {mix64(seed0 ^ (uintptr_t)arg ^ 0xbbbb)};
    sig_ready = 1;
    Set s;
    set_init(&s, live_cap, per_thread_budget());
    long ops = 0;
    while (!(ops % 64 == 0 && done())) {
        if (ops++ % 16 == 0) arm(&r);
        step(&r, &s, xchg_pct);
    }
    if (trap_fd >= 0) close(trap_fd);
    trap_fd = -1;
    sig_ready = 0;
    for (int i = 0; i < RING; i++)
        if (ring_trap[i].p) free_rec(&r, &ring_trap[i]);
    set_free_all(&r, &s);
    set_done(&s);
    return NULL;
}

// The object's functions from SOAK_FUNCS, as offsets from its base, and the sequences the
// sweep expects never to fire, from SOAK_UNREACHED.
typedef struct {
    uint64_t start, size;
    char name[240];
} Func;

typedef struct {
    char name[200], reason[200];
    int ordinal;  // -1: every sequence of the function
    int matched, fired;
} Unreached;

static Func *funcs;
static size_t n_funcs;
static Unreached unreached[32];
static size_t n_unreached;

static void load_funcs(void) {
    const char *path = getenv("SOAK_FUNCS");
    if (!path) return;
    FILE *f = fopen(path, "r");
    if (!f) die("cannot open SOAK_FUNCS file %s", path);
    size_t cap = 1 << 15;
    funcs = map(cap * sizeof *funcs);
    char line[1024];
    while (n_funcs < cap && fgets(line, sizeof line, f)) {
        unsigned long long a, n;
        char type;
        int used = 0;
        if (sscanf(line, "%llx %llx %c %n", &a, &n, &type, &used) < 3 || !used) continue;
        if (type != 't' && type != 'T') continue;
        Func *fn = &funcs[n_funcs++];
        fn->start = a;
        fn->size = n;
        snprintf(fn->name, sizeof fn->name, "%s", line + used);
        fn->name[strcspn(fn->name, "\n")] = 0;
    }
    fclose(f);
}

static void load_unreached(void) {
    const char *path = getenv("SOAK_UNREACHED");
    if (!path) return;
    FILE *f = fopen(path, "r");
    if (!f) die("cannot open SOAK_UNREACHED file %s", path);
    char line[512];
    while (n_unreached < 32 && fgets(line, sizeof line, f)) {
        line[strcspn(line, "\n")] = 0;
        char *bar = strchr(line, '|');
        if (line[0] == '#' || !bar) continue;
        *bar = 0;
        Unreached *u = &unreached[n_unreached++];
        u->ordinal = -1;
        char *hash = strrchr(line, '#');
        if (hash) {
            *hash = 0;
            u->ordinal = atoi(hash + 1);
        }
        snprintf(u->name, sizeof u->name, "%.199s", line);
        snprintf(u->reason, sizeof u->reason, "%.199s", bar + 1);
    }
    fclose(f);
}

// The name of the function that holds `ip`: from SOAK_FUNCS, else the exported symbol, else
// "?".
static const char *symbol_of(uint64_t ip) {
    uint64_t off = ip - object_base;
    for (size_t i = 0; i < n_funcs; i++)
        if (off - funcs[i].start < funcs[i].size) return funcs[i].name;
    Dl_info info;
    return dladdr((void *)ip, &info) && info.dli_sname ? info.dli_sname : "?";
}

// The entry for the sequence at `start` in the function `name`, which holds `rank` sequences
// below it.
static Unreached *listed_unreached(const char *name, int rank) {
    for (size_t i = 0; i < n_unreached; i++)
        if (strstr(name, unreached[i].name) &&
            (unreached[i].ordinal < 0 || unreached[i].ordinal == rank))
            return &unreached[i];
    return NULL;
}

// sweep: every listed instruction in turn, on the main thread.
static long sweep_ops = 200000;

static void *sweep_background(void *arg) {
    Rng r = {mix64(seed0 ^ (uintptr_t)arg ^ 0xcccc)};
    Set s;
    set_init(&s, live_cap, per_thread_budget());
    long ops = 0;
    while (!(ops++ % 64 == 0 && atomic_load(&stop))) step(&r, &s, xchg_pct);
    set_free_all(&r, &s);
    set_done(&s);
    return NULL;
}

static void run_sweep(void) {
    if (!n_insns) die("the sweep needs SOAK_INSNS and an asmalloc object with sequences");
    load_funcs();
    load_unreached();
    unsigned char *fired = map(n_insns);
    pthread_t *t = map(sizeof(pthread_t) * (size_t)nthreads);
    for (long i = 1; i < nthreads; i++)
        if (pthread_create(&t[i], NULL, sweep_background, (void *)(i + 1)))
            die("pthread_create failed");
    sig_rng.s = mix64(seed0 ^ 0xdddd);
    sig_ready = 1;
    Rng r = {mix64(seed0 ^ 0xeeee)};
    Set s;
    set_init(&s, live_cap, per_thread_budget());
    long hit = 0, ops = 0;
    for (size_t i = 0; i < n_insns; i++) {
        long before = trap_hits;
        arm_at(insns[i]);
        for (long k = 0; k < sweep_ops && trap_hits == before; k++, ops++) step(&r, &s, xchg_pct);
        disarm();
        fired[i] = trap_hits != before;
        hit += fired[i];
    }
    sig_ready = 0;
    for (int i = 0; i < RING; i++)
        if (ring_trap[i].p) free_rec(&r, &ring_trap[i]);
    set_free_all(&r, &s);
    set_done(&s);
    atomic_store(&stop, 1);
    for (long i = 1; i < nthreads; i++) pthread_join(t[i], NULL);
    size_t covered = 0, missed = 0, stale = 0, expected = 0;
    for (size_t q = 0; q < n_sequences; q++) {
        const RseqCs *cs = &sequences[q];
        size_t listed = 0, got = 0;
        for (size_t i = 0; i < n_insns; i++)
            if (insns[i] - cs->start_ip < cs->post_commit_offset) {
                listed++;
                got += fired[i];
            }
        const char *name = symbol_of(cs->start_ip);
        int rank = 0;
        for (size_t o = 0; o < n_sequences; o++)
            if (sequences[o].start_ip < cs->start_ip && symbol_of(sequences[o].start_ip) == name)
                rank++;
        Unreached *u = listed_unreached(name, rank);
        say("soak sweep: sequence at +%#lx in %s#%d: %zu of %zu instructions fired%s%s\n",
            (unsigned long)(cs->start_ip - object_base), name, rank, got, listed,
            u ? "; listed as never running: " : "", u ? u->reason : "");
        if (u) {
            u->matched++;
            expected++;
            if (got) {
                u->fired++;
                stale++;
            }
        } else if (got) {
            covered++;
        } else {
            missed++;
        }
    }
    size_t unmatched = 0;
    for (size_t i = 0; i < n_unreached; i++)
        if (!unreached[i].matched && ++unmatched)
            say("soak sweep: no sequence is in a function named like \"%s\"\n",
                unreached[i].name);
    say("soak sweep: %zu of %zu sequences and %ld of %zu instructions fired, in %ld operations; "
        "%zu listed as never running\n",
        covered + stale, n_sequences, hit, n_insns, ops, expected);
    if (missed) die("%zu sequences never fired", missed);
    if (stale) die("%zu sequences listed as never running fired; update the list", stale);
    if (unmatched) die("%zu entries of the never-running list match no sequence", unmatched);
}

static void run_trap(void) {
    dl_iterate_phdr(find_sequences, NULL);
    if (!n_sequences) {
        if (!strcmp(mode, "sweep")) die("no asmalloc object with restartable sequences to sweep");
        say("soak breakpoint: no asmalloc object with restartable sequences; nothing to do\n");
        return;
    }
    say("soak breakpoint: %zu sequences, %zu instructions in them listed\n", n_sequences,
        n_insns);
    table_on = 0;  // the handler could interrupt its own thread holding a bucket lock
    struct sigaction sa;
    memset(&sa, 0, sizeof sa);
    sa.sa_handler = on_trap;
    sigaction(SIGTRAP, &sa, NULL);
    if (!strcmp(mode, "sweep"))
        run_sweep();
    else
        run_threads(trap_worker, nthreads);
}

// ---- main ----

static void parse(int argc, char **argv) {
    if (argc < 2) {
        say("usage: soak MODE [key=value...]\n");
        exit(2);
    }
    mode = argv[1];
    for (int i = 2; i < argc; i++) {
        char *eq = strchr(argv[i], '=');
        if (!eq) die("bad argument %s", argv[i]);
        *eq = 0;
        const char *k = argv[i], *v = eq + 1;
        if (!strcmp(k, "threads"))
            nthreads = atoi(v);
        else if (!strcmp(k, "secs"))
            secs = atof(v);
        else if (!strcmp(k, "seed"))
            seed0 = strtoull(v, NULL, 0);
        else if (!strcmp(k, "maxsize"))
            maxsize = strtoull(v, NULL, 0);
        else if (!strcmp(k, "live"))
            live_cap = atoi(v);
        else if (!strcmp(k, "xchg"))
            xchg_pct = atoi(v);
        else if (!strcmp(k, "sigus"))
            sig_us = atol(v);
        else if (!strcmp(k, "sweepops"))
            sweep_ops = atol(v);
        else if (!strcmp(k, "quietms"))
            quiet_ms = atol(v);
        else if (!strcmp(k, "quietpct"))
            quiet_pct = atol(v);
        else if (!strcmp(k, "trimus"))
            trim_us = atol(v);
        else if (!strcmp(k, "budget"))
            budget_mb = strtoull(v, NULL, 0);
        else if (!strcmp(k, "usable"))
            use_usable = atoi(v);
        else
            die("unknown key %s", k);
    }
    if (nthreads < 1 || live_cap < 4 || maxsize < 64) die("bad parameters");
}

int main(int argc, char **argv) {
    parse(argc, argv);
    free_sized_fn = (void (*)(void *, size_t))dlsym(RTLD_DEFAULT, "free_sized");
    table = map(sizeof(Bucket) * LT_BUCKETS);
    table_on = 1;
    xs = map(sizeof(XSlot) * XSLOTS);
    sched_getaffinity(0, sizeof allowed, &allowed);
    for (int c = 0; c < CPU_SETSIZE; c++)
        if (CPU_ISSET(c, &allowed)) allowed_cpus[n_allowed++] = c;
    deadline = now() + secs;
    double start = now();

    Rng r = {mix64(seed0 ^ 0xabcd)};
    if (!strcmp(mode, "churn")) {
        run_threads(churn_worker, nthreads);
    } else if (!strcmp(mode, "prodcons")) {
        if (nthreads < 2) nthreads = 2;
        producers_left = (nthreads + 1) / 2;
        run_threads(prodcons_worker, nthreads);
    } else if (!strcmp(mode, "fork")) {
        run_threads(fork_worker, nthreads);
    } else if (!strcmp(mode, "threadstorm")) {
        run_threadstorm();
    } else if (!strcmp(mode, "realloc")) {
        run_threads(realloc_worker, nthreads);
    } else if (!strcmp(mode, "huge")) {
        run_threads(huge_worker, nthreads);
    } else if (!strcmp(mode, "signal")) {
        run_signal();
    } else if (!strcmp(mode, "breakpoint") || !strcmp(mode, "sweep")) {
        run_trap();
    } else if (!strcmp(mode, "quiet")) {
        run_quiet();
    } else if (!strcmp(mode, "trim")) {
        run_trim();
    } else if (!strcmp(mode, "idle")) {
        run_threads(idle_worker, nthreads);
    } else {
        die("unknown mode");
    }
    xdrain(&r);

    say("soak %s: %.1fs alloc %ld free %ld realloc %ld look %ld xchg %ld fork %ld thread %ld "
        "signal %ld trap %ld trim %ld null %ld untracked %ld; held at most %ld MiB\n",
        mode, now() - start, n_alloc, n_free, n_realloc, n_verify, n_xchg, n_fork, n_thread,
        n_signal, n_trap, n_trim, n_null, table_dropped, live_peak >> 20);
    if (n_alloc != n_free) die("%ld blocks allocated but %ld freed", n_alloc, n_free);
    printf("soak %s: ok\n", mode);
    return 0;
}
