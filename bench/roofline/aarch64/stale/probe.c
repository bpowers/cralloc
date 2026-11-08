// How long does a scratch register keep its value between allocator calls in real programs, and
// how often does a call use the same size class as the one before?
//
// Preloaded over the C library's allocator. Each call leaves a sentinel in x9, x15, x17 and x18 as
// it returns, and the next call checks which still hold it at entry. A stand-in for a routine that
// keeps an address in a scratch register and compares it on entry: `kept` is how often the next
// call could use it. For each register two predictors say what the compare's branch would do: two
// bits per call site, and gshare over 14 bits of history. `same` adds that the call's size class
// is the previous call's, which is what keeping the word's address (not the slab's) needs. A real
// core's predictor is better than either. The summary goes to stderr at exit.
//
//   cc -O2 -fPIC -shared -ffixed-x18 probe.c -o probe.so -ldl
//   LD_PRELOAD=$PWD/probe.so PROGRAM ...
#define _GNU_SOURCE
#include <dlfcn.h>
#include <malloc.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define S9 0x5a5a000000000009ul
#define S15 0x5a5a00000000000ful
#define S17 0x5a5a000000000011ul
#define S18 0x5a5a000000000012ul
#define SITES (1 << 16)
#define NOUT 5

static void *(*real_malloc)(size_t);
static void (*real_free)(void *);
static void *(*real_calloc)(size_t, size_t);
static void *(*real_realloc)(void *, size_t);

struct out {
  uint64_t kept, miss_local, miss_gshare;
  uint8_t local[SITES], gshare[SITES];
  uint32_t history;
};
static struct out out[NOUT];
static uint64_t calls;
static int started;

static char early[65536];
static size_t early_used;
static int resolving;

// What the current call's class is, set by the wrappers before the bookkeeping that needs it.
static int last_class = -1;

static int class_of(size_t n) {
  if (n <= 8) return 1;
  if (n <= 64) return (int)((n + 7) / 8);
  int log = 63 - __builtin_clzl(n - 1);
  return 8 + (log - 6) * 4 + (int)(((n - 1) >> (log - 2)) & 3) + 1;
}

static void report(void) {
  const char *names[NOUT] = {"x9", "x15", "x17", "x18", "same"};
  char line[1024];
  int n = snprintf(line, sizeof line, "stale-probe: %llu calls;", (unsigned long long)calls);
  for (int r = 0; r < NOUT; r++) {
    double c = calls ? 100.0 / calls : 0;
    n += snprintf(line + n, sizeof line - n, " %s kept %.1f%% miss %.2f%%/%.2f%%;", names[r], out[r].kept * c,
                  out[r].miss_local * c, out[r].miss_gshare * c);
  }
  line[n++] = '\n';
  fwrite(line, 1, n, stderr);
}

static void resolve(void) {
  if (real_malloc || resolving) return;
  resolving = 1;
  real_malloc = dlsym(RTLD_NEXT, "malloc");
  real_free = dlsym(RTLD_NEXT, "free");
  real_calloc = dlsym(RTLD_NEXT, "calloc");
  real_realloc = dlsym(RTLD_NEXT, "realloc");
  resolving = 0;
  atexit(report);
}

static void step(struct out *o, unsigned site, int taken) {
  o->kept += taken;
  uint8_t *c = &o->local[site & (SITES - 1)];
  if ((*c >= 2) != (taken != 0)) o->miss_local++;
  if (taken && *c < 3) ++*c;
  if (!taken && *c > 0) --*c;
  c = &o->gshare[(site ^ (o->history & 0x3fff) << 2) & (SITES - 1)];
  if ((*c >= 2) != (taken != 0)) o->miss_gshare++;
  if (taken && *c < 3) ++*c;
  if (!taken && *c > 0) --*c;
  o->history = o->history << 1 | (taken != 0);
}

__attribute__((used)) void probe_record(uint64_t x9, uint64_t x15, uint64_t x17, uint64_t x18, uint64_t site,
                                         size_t size, void *ptr) {
  int regs[4] = {x9 == S9, x15 == S15, x17 == S17, x18 == S18};
  size_t n = ptr ? malloc_usable_size(ptr) : size;
  int class = class_of(n);
  int same = regs[3] && class == last_class;
  last_class = class;
  if (!started) {
    started = 1;
    return;
  }
  calls++;
  unsigned h = (unsigned)((site >> 2) ^ (site >> 18));
  for (int r = 0; r < 4; r++) step(&out[r], h, regs[r]);
  step(&out[4], h, same);
}

// Entry: x9, x15, x17, x18 as the program left them, the return address as the site; x5 and x6 say
// what the call is for. Exit: sentinels.
#define ENTRY(name, inner, size_reg, ptr_reg)                  \
  __asm__(                                                     \
      ".text\n.globl " #name "\n.type " #name ",%function\n" #name ":\n" \
      "stp x29, x30, [sp, #-112]!\n"                           \
      "mov x29, sp\n"                                          \
      "stp x0, x1, [sp, #16]\n"                                \
      "stp x2, x3, [sp, #32]\n"                                \
      "stp x4, x5, [sp, #48]\n"                                \
      "stp x6, x7, [sp, #64]\n"                                \
      "mov x5, " size_reg "\n"                                 \
      "mov x6, " ptr_reg "\n"                                  \
      "mov x0, x9\n"                                           \
      "mov x1, x15\n"                                          \
      "mov x2, x17\n"                                          \
      "mov x3, x18\n"                                          \
      "mov x4, x30\n"                                          \
      "bl probe_record\n"                                      \
      "ldp x0, x1, [sp, #16]\n"                                \
      "ldp x2, x3, [sp, #32]\n"                                \
      "ldp x4, x5, [sp, #48]\n"                                \
      "ldp x6, x7, [sp, #64]\n"                                \
      "bl " #inner "\n"                                        \
      "ldr x9, =0x5a5a000000000009\n"                          \
      "ldr x15, =0x5a5a00000000000f\n"                         \
      "ldr x17, =0x5a5a000000000011\n"                         \
      "ldr x18, =0x5a5a000000000012\n"                         \
      "ldp x29, x30, [sp], #112\n"                             \
      "ret\n"                                                  \
      ".ltorg\n"                                               \
      ".size " #name ", .-" #name "\n")

ENTRY(malloc, probe_malloc, "x0", "xzr");
ENTRY(free, probe_free, "xzr", "x0");
ENTRY(calloc, probe_calloc, "x1", "xzr");
ENTRY(realloc, probe_realloc, "x1", "xzr");

static int in_early(void *p) { return (char *)p >= early && (char *)p < early + sizeof early; }

__attribute__((used)) void *probe_malloc(size_t n) {
  resolve();
  if (!real_malloc) {
    void *p = early + early_used;
    early_used += (n + 15) & ~15ul;
    return p;
  }
  return real_malloc(n);
}

__attribute__((used)) void probe_free(void *p) {
  if (in_early(p)) return;
  resolve();
  if (real_free) real_free(p);
}

__attribute__((used)) void *probe_calloc(size_t a, size_t b) {
  resolve();
  if (!real_calloc) {
    void *p = probe_malloc(a * b);
    memset(p, 0, a * b);
    return p;
  }
  return real_calloc(a, b);
}

__attribute__((used)) void *probe_realloc(void *p, size_t n) {
  resolve();
  if (in_early(p)) {
    void *q = probe_malloc(n);
    memcpy(q, p, n);
    return q;
  }
  return real_realloc(p, n);
}
