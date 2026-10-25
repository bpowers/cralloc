// What the kernel charges for making anonymous pages resident: a fault per page against one
// MADV_POPULATE_WRITE call over the run, on a mapping set up as asmalloc's regions are (reserved
// PROT_NONE and NORESERVE, made writable with mprotect, advised MADV_NOHUGEPAGE).
// For docs/research/first-touch.md.
//
//   cc -O2 -pthread -o populate populate.c
//   populate cost CPU [ROUNDS [PAGES]]  cycles a page by run length, touch and state, in
//                                       batches of PAGES pages
//   populate hold CPU                   how long one call holds the calling thread
//   populate neigh CPU_POP CPU,CPU,...  what a populating thread does to other threads of the
//                                       process faulting in the same mapping, and to an
//                                       mprotect on it
//
// Cycles are the calling thread's core cycles, user and kernel (perf_event_open), over a batch
// of runs, divided by the pages. Run each under scripts/measure and scripts/memlimit, pinned.
//
// States: "fresh" is a new mapping, with no page tables; "returned" is pages given back with
// MADV_DONTNEED while one page of each 2 MiB stays resident, so the page table stays (a run of
// 512 pages is a whole table, which the kernel frees with its pages under CONFIG_PT_RECLAIM, as
// it would in the heap); "resident" is pages already present.
// Touches: "byte" writes one byte a page, "page" writes all 4 KiB (memset).
#define _GNU_SOURCE
#include <errno.h>
#include <linux/perf_event.h>
#include <pthread.h>
#include <sched.h>
#include <stdatomic.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/syscall.h>
#include <time.h>
#include <unistd.h>
#include <x86intrin.h>

#ifndef MADV_POPULATE_WRITE
#define MADV_POPULATE_WRITE 23
#endif

#define PAGE 4096UL
#define HUGE (2UL << 20)

static void pin(int cpu) {
  cpu_set_t s;
  CPU_ZERO(&s);
  CPU_SET(cpu, &s);
  if (sched_setaffinity(0, sizeof s, &s)) {
    perror("sched_setaffinity");
    exit(1);
  }
}

static int counter(uint32_t type, uint64_t config) {
  struct perf_event_attr a;
  memset(&a, 0, sizeof a);
  a.size = sizeof a;
  a.type = type;
  a.config = config;
  int fd = syscall(SYS_perf_event_open, &a, 0, -1, -1, 0);
  if (fd < 0) {
    perror("perf_event_open");
    exit(1);
  }
  return fd;
}

static uint64_t rd(int fd) {
  uint64_t v = 0;
  if (read(fd, &v, sizeof v) != sizeof v) exit(1);
  return v;
}

static uint64_t now_ns(void) {
  struct timespec t;
  clock_gettime(CLOCK_MONOTONIC, &t);
  return (uint64_t)t.tv_sec * 1000000000ULL + t.tv_nsec;
}

// A region as asmalloc makes one: reserved inaccessible, a prefix made writable, advised
// against huge pages. Aligned to 2 MiB.
static char *region(size_t len, char **raw_out, size_t *raw_len) {
  size_t rl = len + HUGE;
  char *raw = mmap(NULL, rl, PROT_NONE, MAP_PRIVATE | MAP_ANONYMOUS | MAP_NORESERVE, -1, 0);
  if (raw == MAP_FAILED) {
    perror("mmap");
    exit(1);
  }
  char *p = (char *)(((uintptr_t)raw + HUGE - 1) & ~(HUGE - 1));
  if (mprotect(p, len, PROT_READ | PROT_WRITE) || madvise(p, len, MADV_NOHUGEPAGE)) {
    perror("mprotect/madvise");
    exit(1);
  }
  *raw_out = raw;
  *raw_len = rl;
  return p;
}

static void populate(char *p, size_t len) {
  if (madvise(p, len, MADV_POPULATE_WRITE)) {
    perror("MADV_POPULATE_WRITE");
    exit(1);
  }
}

static inline void touch(char *p, size_t pages, int whole) {
  for (size_t i = 0; i < pages; i++) {
    if (whole)
      memset(p + i * PAGE, 0x5a, PAGE);
    else
      *(volatile char *)(p + i * PAGE) = 1;
  }
}

static int cmp_u64(const void *a, const void *b) {
  uint64_t x = *(const uint64_t *)a, y = *(const uint64_t *)b;
  return x < y ? -1 : x > y;
}

// --- cost
enum { FRESH, RETURNED, RESIDENT };
enum { OP_FAULT, OP_POP, OP_CALL };
static const char *STATE[] = {"fresh", "returned", "resident"};
static const char *OP[] = {"fault", "populate+touch", "populate-call"};

// The start page of run i of length n in a batch, in a layout where page 0 of each 2 MiB is a
// sentinel kept resident (runs of up to 511 pages) or, for n = 512, the whole 2 MiB.
static size_t run_start(size_t i, size_t n) {
  if (n >= 512) return i * 512;
  size_t per = 511 / n;
  return (i / per) * 512 + 1 + (i % per) * n;
}

static void cost(int cpu, int rounds, size_t batch_pages) {
  pin(cpu);
  int cyc = counter(PERF_TYPE_HARDWARE, PERF_COUNT_HW_CPU_CYCLES);
  int pf = counter(PERF_TYPE_SOFTWARE, PERF_COUNT_SW_PAGE_FAULTS);
  const size_t lens[] = {1, 4, 16, 64, 512};
  printf("# cost: cycles a page (median over %d rounds), faults a page; batches of %zu pages; cpu %d\n",
         rounds, batch_pages, cpu);
  printf("%-9s %-5s %5s %-15s %10s %10s %8s %8s %6s\n", "state", "touch", "run", "op", "cyc/page",
         "p90", "ns/page", "pf/page", "GHz");
  for (int st = FRESH; st <= RESIDENT; st++)
    for (int whole = 0; whole <= 1; whole++)
      for (size_t li = 0; li < sizeof lens / sizeof *lens; li++) {
        size_t n = lens[li], runs = batch_pages / n;
        size_t span = (run_start(runs - 1, n) + n + 511) / 512 * 512 * PAGE;
        uint64_t res[3][64], nss[3][64];
        double pfs[3] = {0, 0, 0}, ghz[3] = {0, 0, 0};
        for (int r = 0; r < rounds; r++)
          for (int o = 0; o < 3; o++) {
            int op = (o + r) % 3; // rotate the order
            if (whole && op == OP_CALL) {
              res[op][r] = nss[op][r] = 0;
              continue;
            }
            char *raw;
            size_t rl;
            char *p = region(span, &raw, &rl);
            if (st != FRESH) {
              populate(p, span);
              if (st == RETURNED) {
                for (size_t i = 0; i < runs; i++)
                  madvise(p + run_start(i, n) * PAGE, n * PAGE, MADV_DONTNEED);
              }
            }
            uint64_t c0 = rd(cyc), f0 = rd(pf), t0 = now_ns();
            for (size_t i = 0; i < runs; i++) {
              char *q = p + run_start(i, n) * PAGE;
              if (op != OP_FAULT) populate(q, n * PAGE);
              if (op != OP_CALL) touch(q, n, whole);
            }
            uint64_t t1 = now_ns(), c1 = rd(cyc), f1 = rd(pf);
            res[op][r] = (c1 - c0) / (runs * n);
            nss[op][r] = (t1 - t0) / (runs * n);
            ghz[op] += (double)(c1 - c0) / (t1 - t0) / rounds;
            pfs[op] += (double)(f1 - f0) / (runs * n) / rounds;
            munmap(raw, rl);
          }
        for (int op = 0; op < 3; op++) {
          if (whole && op == OP_CALL) continue;
          qsort(res[op], rounds, sizeof(uint64_t), cmp_u64);
          qsort(nss[op], rounds, sizeof(uint64_t), cmp_u64);
          printf("%-9s %-5s %5zu %-15s %10lu %10lu %8lu %8.3f %6.2f\n", STATE[st],
                 whole ? "page" : "byte", n, OP[op], res[op][rounds / 2], res[op][rounds * 9 / 10],
                 nss[op][rounds / 2], pfs[op], ghz[op]);
        }
        fflush(stdout);
      }
}

// --- hold: one call of each size, fresh and returned; wall time and cycles.
static void hold(int cpu) {
  pin(cpu);
  int cyc = counter(PERF_TYPE_HARDWARE, PERF_COUNT_HW_CPU_CYCLES);
  const size_t sizes[] = {64UL << 10, 256UL << 10, 1UL << 20, 2UL << 20, 8UL << 20, 32UL << 20,
                          64UL << 20, 256UL << 20};
  printf("# hold: one MADV_POPULATE_WRITE call, median and max of 9; cpu %d\n", cpu);
  printf("%-9s %10s %10s %10s %10s %10s\n", "state", "KiB", "us", "max us", "Mcyc", "cyc/page");
  for (int st = FRESH; st <= RETURNED; st++)
    for (size_t si = 0; si < sizeof sizes / sizeof *sizes; si++) {
      size_t len = sizes[si];
      uint64_t ns[9], cs[9];
      for (int r = 0; r < 9; r++) {
        char *raw;
        size_t rl;
        char *p = region(len, &raw, &rl);
        if (st == RETURNED) {
          populate(p, len);
          madvise(p, len, MADV_DONTNEED);
          // Keep the page tables, as a heap would around a purged run.
          for (size_t o = 0; o < len; o += HUGE) *(volatile char *)(p + o) = 1;
        }
        uint64_t c0 = rd(cyc), t0 = now_ns();
        populate(p, len);
        uint64_t t1 = now_ns(), c1 = rd(cyc);
        ns[r] = t1 - t0;
        cs[r] = c1 - c0;
        munmap(raw, rl);
      }
      qsort(ns, 9, sizeof(uint64_t), cmp_u64);
      qsort(cs, 9, sizeof(uint64_t), cmp_u64);
      printf("%-9s %10zu %10.1f %10.1f %10.2f %10lu\n", STATE[st], len >> 10, ns[4] / 1e3,
             ns[8] / 1e3, cs[4] / 1e6, cs[4] / (len / PAGE));
      fflush(stdout);
    }
}

// --- neigh: faulting threads beside one that populates, faults, or idles.
static atomic_int stop, go;
static char *area;
static size_t area_len;
static size_t pop_chunk;
static int pop_mode; // 0 idle (spins in user mode), 1 faults its chunks, 2 populates them
static atomic_ulong pop_bytes;

#define NLAT 200000
struct faulter {
  int cpu;
  char *base;
  size_t len;
  uint64_t *lat;
  size_t nlat;
  uint64_t faults;
};

static void *faulter_main(void *arg) {
  struct faulter *f = arg;
  pin(f->cpu);
  while (!atomic_load(&go)) _mm_pause();
  size_t pages = f->len / PAGE, i = 0;
  while (!atomic_load(&stop)) {
    char *q = f->base + i * PAGE;
    uint64_t t0 = __rdtsc();
    *(volatile char *)q = 1;
    uint64_t t1 = __rdtsc();
    if (f->nlat < NLAT) f->lat[f->nlat++] = t1 - t0;
    f->faults++;
    if (++i == pages) {
      madvise(f->base, f->len, MADV_DONTNEED);
      i = 0;
    }
  }
  return NULL;
}

static void *pop_main(void *arg) {
  int cpu = *(int *)arg;
  pin(cpu);
  while (!atomic_load(&go)) _mm_pause();
  size_t off = 0;
  volatile uint64_t spin = 0;
  (void)spin;
  while (!atomic_load(&stop)) {
    if (pop_mode == 0) {
      spin++;
      continue;
    }
    char *q = area + off;
    if (pop_mode == 2)
      populate(q, pop_chunk);
    else
      touch(q, pop_chunk / PAGE, 0);
    atomic_fetch_add(&pop_bytes, pop_chunk);
    off += pop_chunk;
    if (off + pop_chunk > area_len) {
      madvise(area, area_len, MADV_DONTNEED);
      off = 0;
    }
  }
  return NULL;
}

// Toggles one page at the end of the populater's area, as a region's growth step changes the
// protection next to the pages in use; each call takes the address space's lock for writing.
static char *guard;
static uint64_t mp_lat[NLAT];
static size_t mp_n;
static int mp_cpu;
static void *mprot_main(void *arg) {
  (void)arg;
  pin(mp_cpu);
  while (!atomic_load(&go)) _mm_pause();
  int prot = PROT_NONE;
  while (!atomic_load(&stop)) {
    prot = prot == PROT_NONE ? PROT_READ | PROT_WRITE : PROT_NONE;
    uint64_t t0 = now_ns();
    mprotect(guard, PAGE, prot);
    uint64_t t1 = now_ns();
    if (mp_n < NLAT) mp_lat[mp_n++] = t1 - t0;
    // About a thousand a second.
    while (now_ns() - t1 < 1000000) _mm_pause();
  }
  return NULL;
}

static void neigh(int popcpu, const char *cpus) {
  int fc[64], nf = 0;
  for (const char *s = cpus; *s && nf < 64;) {
    fc[nf++] = atoi(s);
    while (*s && *s != ',') s++;
    if (*s == ',') s++;
  }
  mp_cpu = fc[nf - 1];
  nf--; // the last CPU runs the mprotect thread
  const size_t chunks[] = {64UL << 10, 2UL << 20, 16UL << 20};
  const char *modes[] = {"idle", "faults", "populates"};
  size_t flen = 16UL << 20;
  area_len = 256UL << 20;
  printf("# neigh: %d faulters, populater on %d, mprotect on %d; fault latency in TSC ticks, "
         "mprotect latency in us\n",
         nf, popcpu, mp_cpu);
  printf("%-9s %8s %10s %10s %10s %10s %12s %10s %10s %10s %10s\n", "popper", "chunkKiB",
         "flt p50", "flt p99", "flt p99.9", "flt max", "faults/s/thr", "pop MiB/s", "mp p50",
         "mp p99", "mp max");
  for (size_t ci = 0; ci < 3; ci++)
    for (int m = 0; m < 3; m++) {
      if (m == 0 && ci > 0) continue;
      char *raw;
      size_t rl;
      // One mapping: the populater's area, the faulters' areas, and the guard page after them.
      size_t total = area_len + nf * flen + HUGE;
      char *p = region(total, &raw, &rl);
      area = p;
      guard = p + total - PAGE;
      mprotect(guard, PAGE, PROT_NONE);
      pop_mode = m;
      pop_chunk = chunks[ci];
      atomic_store(&pop_bytes, 0);
      atomic_store(&stop, 0);
      atomic_store(&go, 0);
      mp_n = 0;
      pthread_t th[66];
      struct faulter fs[64];
      for (int i = 0; i < nf; i++) {
        fs[i] = (struct faulter){fc[i], p + area_len + i * flen, flen,
                                 malloc(NLAT * sizeof(uint64_t)), 0, 0};
        pthread_create(&th[i], NULL, faulter_main, &fs[i]);
      }
      pthread_create(&th[nf], NULL, pop_main, &popcpu);
      pthread_create(&th[nf + 1], NULL, mprot_main, NULL);
      usleep(50000);
      uint64_t t0 = now_ns();
      atomic_store(&go, 1);
      usleep(2000000);
      atomic_store(&stop, 1);
      uint64_t t1 = now_ns();
      for (int i = 0; i < nf + 2; i++) pthread_join(th[i], NULL);
      double secs = (t1 - t0) / 1e9;
      // Latencies of all faulters together.
      size_t tot = 0;
      uint64_t faults = 0;
      for (int i = 0; i < nf; i++) tot += fs[i].nlat, faults += fs[i].faults;
      uint64_t *all = malloc(tot * sizeof(uint64_t));
      size_t k = 0;
      for (int i = 0; i < nf; i++) {
        memcpy(all + k, fs[i].lat, fs[i].nlat * sizeof(uint64_t));
        k += fs[i].nlat;
        free(fs[i].lat);
      }
      qsort(all, tot, sizeof(uint64_t), cmp_u64);
      qsort(mp_lat, mp_n, sizeof(uint64_t), cmp_u64);
      printf("%-9s %8zu %10lu %10lu %10lu %10lu %12.0f %10.0f %10.1f %10.1f %10.1f\n", modes[m],
             m ? chunks[ci] >> 10 : 0, all[tot / 2], all[tot * 99 / 100], all[tot * 999 / 1000],
             all[tot - 1], faults / secs / nf, atomic_load(&pop_bytes) / secs / (1 << 20),
             mp_lat[mp_n / 2] / 1e3, mp_lat[mp_n * 99 / 100] / 1e3, mp_lat[mp_n - 1] / 1e3);
      fflush(stdout);
      free(all);
      munmap(raw, rl);
    }
}

int main(int argc, char **argv) {
  if (argc < 3) {
    fprintf(stderr, "usage: populate cost CPU [ROUNDS] | hold CPU | neigh CPU_POP CPU,CPU,...\n");
    return 2;
  }
  if (!strcmp(argv[1], "cost")) {
    int rounds = argc > 3 ? atoi(argv[3]) : 15;
    if (rounds > 64) rounds = 64;
    // 8192 pages (32 MiB) a batch by default, more than the L3 holds; 512 fits the L2.
    size_t batch = argc > 4 ? strtoul(argv[4], NULL, 0) : 8192;
    if (batch < 512) batch = 512;
    cost(atoi(argv[2]), rounds, batch);
  } else if (!strcmp(argv[1], "hold")) {
    hold(atoi(argv[2]));
  } else if (!strcmp(argv[1], "neigh") && argc > 3) {
    neigh(atoi(argv[2]), argv[3]);
  } else {
    return 2;
  }
  return 0;
}
