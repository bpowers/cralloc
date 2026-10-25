// What Linux does with part of a transparent huge page returned, and what advice changes
// cost, for docs/research/huge-tail.md. No allocator: plain mappings shaped like a region.
//
//   cc -O2 -pthread -o kernel kernel.c
//   scripts/memlimit -m 1G -- ./kernel partial      (and the other experiments below)
//
// Each experiment prints what the process, its cgroup and the kernel's counters say before
// and after each step. "unit" is 2 MiB, "page" 4 KiB. The cgroup is the one the process runs
// in, which scripts/memlimit makes for it.
//
//   partial UNITS KEEP          fault UNITS huge pages, return all but KEEP pages of each
//                               with MADV_DONTNEED, watch for a minute, then ask the cgroup
//                               to reclaim (memory.reclaim)
//   pressure STRAND KEEP FRESH  strand STRAND MiB as in `partial`, then fault FRESH MiB of
//                               4 KiB pages (or huge ones, with HUGE=1), timing each fault;
//                               run under a cap below STRAND + FRESH
//   split METHOD UNITS KEEP     return all but KEEP pages of each unit, forcing the split by
//                               METHOD (none, cold, free, pageout, after-cold, pm-cold);
//                               SPIN=N keeps N other threads of the process running
//   khuge UNITS KEEP SECS       split and return a tail, advise it NOHUGEPAGE and watch it for
//                               SECS; advise it HUGEPAGE again and watch again, faulting
//                               fresh units and timing MADV_COLLAPSE
//   stall SECS TAIL_MIB GAP_US WHERE FAULT
//                               fault in a loop on two threads (one in the region whose tail
//                               advice flips, one in another mapping) while a third flips the
//                               tail's advice every GAP_US (0: no flips, the baseline).
//                               WHERE is the faulting thread's place in the region, FAULT 4k
//                               or 2m: 4k faults in a part advised against huge pages, 2m in
//                               the advised part next to the tail
//   maps                        the mappings a region leaves in each advice state
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <pthread.h>
#include <sched.h>
#include <stdatomic.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/syscall.h>
#include <sys/uio.h>
#include <time.h>
#include <unistd.h>

#ifndef MADV_COLLAPSE
#define MADV_COLLAPSE 25
#endif
#ifndef MADV_COLD
#define MADV_COLD 20
#endif
#ifndef MADV_PAGEOUT
#define MADV_PAGEOUT 21
#endif

#define PAGE 4096UL
#define UNIT (2UL << 20)
#define PPU (UNIT / PAGE)

static char cgdir[512];

static double now_us(void) {
  struct timespec t;
  clock_gettime(CLOCK_MONOTONIC, &t);
  return t.tv_sec * 1e6 + t.tv_nsec / 1e3;
}

static long file_long(const char *path) {
  FILE *f = fopen(path, "r");
  if (!f) return -1;
  long v = -1;
  if (fscanf(f, "%ld", &v) != 1) v = -1;
  fclose(f);
  return v;
}

// A "key value" line from a file like memory.stat or /proc/vmstat.
static long keyed(const char *path, const char *key) {
  FILE *f = fopen(path, "r");
  if (!f) return -1;
  char line[256];
  size_t n = strlen(key);
  long v = -1;
  while (fgets(line, sizeof line, f)) {
    if (!strncmp(line, key, n) && line[n] == ' ') {
      v = atol(line + n + 1);
      break;
    }
  }
  fclose(f);
  return v;
}

static void cg_init(void) {
  FILE *f = fopen("/proc/self/cgroup", "r");
  char line[512];
  cgdir[0] = 0;
  while (f && fgets(line, sizeof line, f)) {
    if (!strncmp(line, "0::", 3)) {
      line[strcspn(line, "\n")] = 0;
      snprintf(cgdir, sizeof cgdir, "/sys/fs/cgroup%s", line + 3);
    }
  }
  if (f) fclose(f);
}

static long cg(const char *file) {
  char p[640];
  snprintf(p, sizeof p, "%s/%s", cgdir, file);
  return file_long(p);
}

static long cg_stat(const char *key) {
  char p[640];
  snprintf(p, sizeof p, "%s/memory.stat", cgdir);
  return keyed(p, key);
}

static long cg_event(const char *key) {
  char p[640];
  snprintf(p, sizeof p, "%s/memory.events", cgdir);
  return keyed(p, key);
}

// Rss and AnonHugePages (KiB) of the mappings that overlap [a, b), or of the whole process
// when a is NULL.
static void smaps(void *a, void *b, long *rss, long *huge) {
  FILE *f = fopen("/proc/self/smaps", "r");
  char line[512];
  int in = 0;
  *rss = *huge = 0;
  while (fgets(line, sizeof line, f)) {
    unsigned long lo, hi;
    char perm[8];
    if (sscanf(line, "%lx-%lx %7s", &lo, &hi, perm) == 3) {
      in = !a || (lo < (unsigned long)b && hi > (unsigned long)a);
      continue;
    }
    if (!in) continue;
    if (!strncmp(line, "Rss:", 4)) *rss += atol(line + 4);
    if (!strncmp(line, "AnonHugePages:", 14)) *huge += atol(line + 14);
  }
  fclose(f);
}

static int maps_lines(void) {
  FILE *f = fopen("/proc/self/maps", "r");
  int n = 0, c;
  while ((c = fgetc(f)) != EOF) n += c == '\n';
  fclose(f);
  return n;
}

static long vm(const char *key) { return keyed("/proc/vmstat", key); }

static long partially_mapped(void) {
  return file_long("/sys/kernel/mm/transparent_hugepage/hugepages-2048kB/stats/nr_anon_partially_mapped");
}

struct snap {
  long rss, huge, cur, anon, anon_thp, split, dsplit, splitpmd, pm, collapse;
};

static struct snap take(void *a, void *b) {
  struct snap s;
  smaps(a, b, &s.rss, &s.huge);
  s.cur = cg("memory.current");
  s.anon = cg_stat("anon");
  s.anon_thp = cg_stat("anon_thp");
  s.split = vm("thp_split_page");
  s.dsplit = vm("thp_deferred_split_page");
  s.splitpmd = vm("thp_split_pmd");
  s.pm = partially_mapped();
  s.collapse = cg_stat("thp_collapse_alloc");
  return s;
}

static void show(const char *what, struct snap s, struct snap base) {
  printf("  %-34s range rss %7.1f MiB huge %7.1f | cg current %7.1f anon %7.1f anon_thp %7.1f MiB"
         " | +split %ld +deferred %ld +split_pmd %ld partially_mapped(global) %ld +collapse %ld\n",
         what, s.rss / 1024.0, s.huge / 1024.0, s.cur / 1048576.0, s.anon / 1048576.0,
         s.anon_thp / 1048576.0, s.split - base.split, s.dsplit - base.dsplit,
         s.splitpmd - base.splitpmd, s.pm, s.collapse - base.collapse);
  fflush(stdout);
}

static char *map_units(size_t units) {
  size_t len = units * UNIT;
  char *raw = mmap(NULL, len + UNIT, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
  if (raw == MAP_FAILED) {
    perror("mmap");
    exit(1);
  }
  char *p = (char *)(((uintptr_t)raw + UNIT - 1) & ~(UNIT - 1));
  // Trim so the mapping is exactly the aligned range and no neighbour merges with it.
  if (p > raw) munmap(raw, p - raw);
  munmap(p + len, raw + UNIT - p);
  return p;
}

static void touch(char *p, size_t len) {
  for (size_t i = 0; i < len; i += PAGE) p[i] = 1;
}

static void must(int r, const char *what) {
  if (r) {
    fprintf(stderr, "%s: %s\n", what, strerror(errno));
    exit(1);
  }
}

// Returns the free part of each unit (all but its first `keep` pages).
static double give_back(char *p, size_t units, size_t keep) {
  double t0 = now_us();
  for (size_t u = 0; u < units; u++)
    must(madvise(p + u * UNIT + keep * PAGE, UNIT - keep * PAGE, MADV_DONTNEED), "dontneed");
  return now_us() - t0;
}

static char *fault_huge(size_t units) {
  char *p = map_units(units);
  must(madvise(p, units * UNIT, MADV_HUGEPAGE), "hugepage");
  touch(p, units * UNIT);
  return p;
}

static int try_reclaim(const char *amount) {
  char path[640];
  snprintf(path, sizeof path, "%s/memory.reclaim", cgdir);
  int fd = open(path, O_WRONLY);
  if (fd < 0) {
    printf("  memory.reclaim: cannot open: %s\n", strerror(errno));
    return -1;
  }
  double t0 = now_us();
  ssize_t w = write(fd, amount, strlen(amount));
  int e = errno;
  double t1 = now_us();
  close(fd);
  printf("  memory.reclaim %s: %s in %.0f us\n", amount, w < 0 ? strerror(e) : "ok", t1 - t0);
  return w < 0 ? -1 : 0;
}

static void exp_partial(int argc, char **argv) {
  size_t units = argc > 0 ? atol(argv[0]) : 64, keep = argc > 1 ? atol(argv[1]) : 3;
  printf("partial: %zu units, keep %zu pages each\n", units, keep);
  struct snap b = take(NULL, NULL);
  char *p = fault_huge(units);
  char *e = p + units * UNIT;
  show("faulted", take(p, e), b);
  double us = give_back(p, units, keep);
  printf("  dontneed of the free part of each unit: %.1f us a unit\n", us / units);
  // With OFF=1 the range is advised against huge pages after the return, so that khugepaged
  // leaves it and only the deferred split can free anything.
  if (getenv("OFF") && atoi(getenv("OFF"))) {
    must(madvise(p, units * UNIT, MADV_NOHUGEPAGE), "nohugepage");
    printf("  advised MADV_NOHUGEPAGE after the return\n");
  }
  show("at once", take(p, e), b);
  int waits[] = {5, 25, 30};
  int total = 0;
  // NOWAIT=1 goes straight to the reclaim.
  for (int i = 0; i < (getenv("NOWAIT") ? 0 : 3); i++) {
    sleep(waits[i]);
    total += waits[i];
    char what[64];
    snprintf(what, sizeof what, "after %d s", total);
    show(what, take(p, e), b);
  }
  try_reclaim("1M");
  show("after memory.reclaim 1M", take(p, e), b);
  try_reclaim("64M");
  show("after memory.reclaim 64M", take(p, e), b);
}

static int cmp_d(const void *a, const void *b) {
  double x = *(const double *)a, y = *(const double *)b;
  return x < y ? -1 : x > y;
}

static void latency_summary(const char *what, double *lat, size_t n) {
  if (!n) return;
  qsort(lat, n, sizeof *lat, cmp_d);
  double sum = 0;
  size_t over100 = 0, over1000 = 0, over10000 = 0;
  for (size_t i = 0; i < n; i++) {
    sum += lat[i];
    over100 += lat[i] > 100;
    over1000 += lat[i] > 1000;
    over10000 += lat[i] > 10000;
  }
  printf("  %-26s n %zu mean %.2f us p50 %.2f p99 %.2f p99.9 %.1f max %.0f | >100us %zu >1ms %zu >10ms %zu"
         " | total %.1f ms\n",
         what, n, sum / n, lat[n / 2], lat[n * 99 / 100], lat[n * 999 / 1000], lat[n - 1], over100,
         over1000, over10000, sum / 1000);
}

static void exp_pressure(int argc, char **argv) {
  size_t strand = argc > 0 ? atol(argv[0]) : 256, keep = argc > 1 ? atol(argv[1]) : 3;
  size_t fresh = argc > 2 ? atol(argv[2]) : 512;
  int huge = getenv("HUGE") && atoi(getenv("HUGE"));
  printf("pressure: strand %zu MiB keeping %zu pages a unit, then fault %zu MiB fresh (%s), memory.max %.0f MiB\n",
         strand, keep, fresh, huge ? "huge" : "4 KiB", cg("memory.max") / 1048576.0);
  size_t units = strand / 2;
  struct snap b = take(NULL, NULL);
  char *p = fault_huge(units);
  give_back(p, units, keep);
  show("stranded", take(NULL, NULL), b);
  long max0 = cg_event("max"), oom0 = cg_event("oom");
  size_t len = fresh << 20;
  char *q = map_units(len / UNIT);
  must(madvise(q, len, huge ? MADV_HUGEPAGE : MADV_NOHUGEPAGE), "advise fresh");
  size_t step = huge ? UNIT : PAGE, n = len / step;
  double *lat = malloc(n * sizeof *lat);
  long fb0 = vm("thp_fault_fallback_charge"), fa0 = vm("thp_fault_alloc");
  double t0 = now_us();
  // Report memory.current as the fresh memory crosses each tenth.
  for (size_t i = 0; i < n; i++) {
    double a = now_us();
    q[i * step] = 1;
    lat[i] = now_us() - a;
    if ((i + 1) % (n / 10) == 0) {
      struct snap s = take(NULL, NULL);
      printf("  fresh %5.0f MiB: cg current %6.1f MiB, stranded range rss %6.1f MiB, +split %ld, partially_mapped %ld\n",
             (double)(i + 1) * step / 1048576.0, s.cur / 1048576.0, 0.0, s.split - b.split, s.pm);
      fflush(stdout);
    }
  }
  double t1 = now_us();
  latency_summary("fresh faults", lat, n);
  printf("  fresh total %.1f ms; memory.events max +%ld oom +%ld; thp_fault_alloc +%ld (global) "
         "fallback_charge +%ld\n",
         (t1 - t0) / 1000, cg_event("max") - max0, cg_event("oom") - oom0, vm("thp_fault_alloc") - fa0,
         vm("thp_fault_fallback_charge") - fb0);
  show("end", take(NULL, NULL), b);
}

// One call that makes the kernel split the huge page under a unit, for each method.
static int split_call(const char *method, char *unit, size_t keep) {
  char *first_free = unit + keep * PAGE;
  if (!strcmp(method, "cold")) return madvise(first_free, PAGE, MADV_COLD);
  if (!strcmp(method, "free")) return madvise(first_free, PAGE, MADV_FREE);
  if (!strcmp(method, "pageout")) return madvise(first_free, PAGE, MADV_PAGEOUT);
  return 0;
}

static atomic_int spin_stop;

static void *spinner(void *arg) {
  char *page = arg;
  while (!atomic_load_explicit(&spin_stop, memory_order_relaxed)) page[0]++;
  return NULL;
}

static void exp_split(int argc, char **argv) {
  const char *method = argc > 0 ? argv[0] : "cold";
  size_t units = argc > 1 ? atol(argv[1]) : 64, keep = argc > 2 ? atol(argv[2]) : 3;
  int spin = getenv("SPIN") ? atoi(getenv("SPIN")) : 0;
  printf("split: method %s, %zu units, keep %zu pages, %d other threads running\n", method, units,
         keep, spin);
  pthread_t th[64];
  char *spinpages = map_units(1);
  for (int i = 0; i < spin; i++) pthread_create(&th[i], NULL, spinner, spinpages + i * 64 * 64);
  struct snap b = take(NULL, NULL);
  char *p = fault_huge(units);
  char *e = p + units * UNIT;
  show("faulted", take(p, e), b);
  double *split_us = calloc(units, sizeof(double)), *free_us = calloc(units, sizeof(double));
  if (!strcmp(method, "pm-cold")) {
    // One process_madvise call carries a page of every unit.
    struct iovec *iov = calloc(units, sizeof *iov);
    for (size_t u = 0; u < units; u++) iov[u] = (struct iovec){p + u * UNIT + keep * PAGE, PAGE};
    int pidfd = syscall(SYS_pidfd_open, getpid(), 0);
    double t0 = now_us();
    long r = syscall(SYS_process_madvise, pidfd, iov, units, MADV_COLD, 0);
    double t1 = now_us();
    printf("  process_madvise(MADV_COLD) of %zu pages: %ld, %.1f us (%.1f us a unit)\n", units, r,
           t1 - t0, (t1 - t0) / units);
    for (size_t u = 0; u < units; u++) {
      double a = now_us();
      must(madvise(p + u * UNIT + keep * PAGE, UNIT - keep * PAGE, MADV_DONTNEED), "dontneed");
      free_us[u] = now_us() - a;
    }
  } else {
    for (size_t u = 0; u < units; u++) {
      char *unit = p + u * UNIT;
      double a = now_us();
      must(split_call(method, unit, keep), method);
      double m = now_us();
      must(madvise(unit + keep * PAGE, UNIT - keep * PAGE, MADV_DONTNEED), "dontneed");
      double z = now_us();
      // A split after the return: the folio is already mapped by page tables, partly.
      if (!strcmp(method, "after-cold")) must(madvise(unit, UNIT, MADV_COLD), "after-cold");
      double c = now_us();
      split_us[u] = (m - a) + (c - z);
      free_us[u] = z - m;
    }
  }
  show("at once", take(p, e), b);
  latency_summary("split call", split_us, units);
  latency_summary("dontneed call", free_us, units);
  // The live pages must still read what was written.
  for (size_t u = 0; u < units; u++)
    for (size_t k = 0; k < keep; k++)
      if (p[u * UNIT + k * PAGE] != 1) {
        printf("  LIVE PAGE LOST at unit %zu page %zu\n", u, k);
        exit(1);
      }
  atomic_store(&spin_stop, 1);
  for (int i = 0; i < spin; i++) pthread_join(th[i], NULL);
}

static void watch(char *p, char *e, int secs, struct snap b, const char *label) {
  long h0 = -1;
  for (int t = 0; t <= secs; t += 5) {
    struct snap s = take(p, e);
    if (s.huge != h0) {
      char what[96];
      snprintf(what, sizeof what, "%s t=%d s", label, t);
      show(what, s, b);
      h0 = s.huge;
    }
    if (t < secs) sleep(5);
  }
  char what[96];
  snprintf(what, sizeof what, "%s end (%d s)", label, secs);
  show(what, take(p, e), b);
}

static void exp_khuge(int argc, char **argv) {
  size_t units = argc > 0 ? atol(argv[0]) : 32, keep = argc > 1 ? atol(argv[1]) : 3;
  int secs = argc > 2 ? atoi(argv[2]) : 120;
  printf("khuge: %zu units (half kept partly, half returned whole), keep %zu, watch %d s; "
         "khugepaged pages_to_scan %ld every %ld ms\n",
         units, keep, secs,
         file_long("/sys/kernel/mm/transparent_hugepage/khugepaged/pages_to_scan"),
         file_long("/sys/kernel/mm/transparent_hugepage/khugepaged/scan_sleep_millisecs"));
  struct snap b = take(NULL, NULL);
  char *p = fault_huge(units);
  char *e = p + units * UNIT;
  for (size_t u = 0; u < units; u++) {
    char *unit = p + u * UNIT;
    must(madvise(unit + keep * PAGE, PAGE, MADV_COLD), "cold");
    // Even units keep their first pages; odd ones are returned whole.
    size_t k = u % 2 ? 0 : keep;
    must(madvise(unit + k * PAGE, UNIT - k * PAGE, MADV_DONTNEED), "dontneed");
  }
  double t0 = now_us();
  must(madvise(p, units * UNIT, MADV_NOHUGEPAGE), "nohugepage");
  printf("  MADV_NOHUGEPAGE on the tail: %.1f us\n", now_us() - t0);
  show("split, returned, advised against", take(p, e), b);
  watch(p, e, secs, b, "advice off");
  t0 = now_us();
  must(madvise(p, units * UNIT, MADV_HUGEPAGE), "hugepage");
  printf("  MADV_HUGEPAGE on the tail: %.1f us\n", now_us() - t0);
  // A fault in a unit that was returned whole: does it come as a huge page at once?
  size_t whole_huge = 0, part_huge = 0;
  for (size_t u = 1; u < units / 2; u += 2) {
    double a = now_us();
    p[u * UNIT] = 1;
    double us = now_us() - a;
    long rss, huge;
    smaps(p + u * UNIT, p + (u + 1) * UNIT, &rss, &huge);
    // smaps sums the whole mapping; compare with the unit before.
    if (u == 1) printf("  first fault in a unit returned whole: %.1f us\n", us);
    (void)rss;
    (void)huge;
  }
  struct snap s = take(p, e);
  printf("  after one fault in each of %zu units returned whole: range huge %.1f MiB\n",
         (units / 2 + 1) / 2, s.huge / 1024.0);
  whole_huge = s.huge;
  // A fault in a unit kept partly.
  p[2 * UNIT + 100 * PAGE] = 1;
  s = take(p, e);
  part_huge = s.huge;
  printf("  after a fault in a unit kept partly: range huge %.1f MiB (was %.1f)\n", part_huge / 1024.0,
         whole_huge / 1024.0);
  // Time a synchronous collapse of one unit kept partly, then let khugepaged have the rest.
  t0 = now_us();
  int r = madvise(p + 4 * UNIT, UNIT, MADV_COLLAPSE);
  printf("  MADV_COLLAPSE of one unit kept partly: %s, %.0f us\n", r ? strerror(errno) : "ok", now_us() - t0);
  watch(p, e, secs, b, "advice on");
}

struct faulter {
  char *base;
  size_t len, step;
  int secs;
  double *lat;
  size_t n, cap;
  int cpu;
};

static atomic_int stall_go, stall_stop;

static void pin(int cpu) {
  if (cpu < 0) return;
  cpu_set_t s;
  CPU_ZERO(&s);
  CPU_SET(cpu, &s);
  sched_setaffinity(0, sizeof s, &s);
}

static void *fault_loop(void *arg) {
  struct faulter *f = arg;
  pin(f->cpu);
  while (!atomic_load(&stall_go)) {
  }
  while (!atomic_load_explicit(&stall_stop, memory_order_relaxed)) {
    for (size_t off = 0; off < f->len && !atomic_load_explicit(&stall_stop, memory_order_relaxed);
         off += f->step) {
      double a = now_us();
      f->base[off] = 1;
      double d = now_us() - a;
      if (f->n < f->cap) f->lat[f->n++] = d;
    }
    madvise(f->base, f->len, MADV_DONTNEED);
  }
  return NULL;
}

static void exp_stall(int argc, char **argv) {
  int secs = argc > 0 ? atoi(argv[0]) : 5;
  size_t tail_mib = argc > 1 ? atol(argv[1]) : 1024;
  int gap = argc > 2 ? atoi(argv[2]) : 1000;
  const char *fault = argc > 3 ? argv[3] : "4k";
  int cpu0 = getenv("CPU0") ? atoi(getenv("CPU0")) : -1;
  int big = !strcmp(fault, "2m");
  // The region: a head of 64 MiB, then the tail whose advice flips. For 4k faults the head is
  // advised against huge pages and the tail flips between that and HUGEPAGE, so the faulting
  // thread's mapping is split and merged by every flip. For 2m faults the head is advised for
  // huge pages and the tail flips between NOHUGEPAGE and HUGEPAGE.
  size_t head = 64UL << 20, tail = tail_mib << 20;
  char *r = map_units((head + tail) / UNIT);
  must(madvise(r, head, big ? MADV_HUGEPAGE : MADV_NOHUGEPAGE), "head");
  must(madvise(r + head, tail, MADV_HUGEPAGE), "tail");
  touch(r + head, tail);
  long rss, huge;
  smaps(r + head, r + head + tail, &rss, &huge);
  char *other = map_units(head / UNIT);
  must(madvise(other, head, big ? MADV_HUGEPAGE : MADV_NOHUGEPAGE), "other");
  printf("stall: %d s, tail %zu MiB (%.0f MiB of it in huge pages), flip every %d us (0: none), %s faults,"
         " maps lines %d\n",
         secs, tail_mib, huge / 1024.0, gap, fault, maps_lines());
  size_t cap = 50 * 1000 * 1000 / sizeof(double);
  struct faulter fs[2] = {
      {r, head, big ? UNIT : PAGE, secs, malloc(cap * sizeof(double)), 0, cap, cpu0 >= 0 ? cpu0 + 1 : -1},
      {other, head, big ? UNIT : PAGE, secs, malloc(cap * sizeof(double)), 0, cap, cpu0 >= 0 ? cpu0 + 2 : -1},
  };
  pthread_t th[2];
  for (int i = 0; i < 2; i++) pthread_create(&th[i], NULL, fault_loop, &fs[i]);
  pin(cpu0);
  size_t flips = 0, fcap = 10 * 1000 * 1000;
  double *flip = malloc(fcap * sizeof(double));
  atomic_store(&stall_go, 1);
  double end = now_us() + secs * 1e6;
  int on = 1;
  while (now_us() < end) {
    if (gap) {
      double a = now_us();
      must(madvise(r + head, tail, on ? MADV_NOHUGEPAGE : MADV_HUGEPAGE), "flip");
      if (flips < fcap) flip[flips++] = now_us() - a;
      on = !on;
      double until = now_us() + gap;
      while (now_us() < until) {
      }
    } else {
      usleep(10000);
    }
  }
  atomic_store(&stall_stop, 1);
  for (int i = 0; i < 2; i++) pthread_join(th[i], NULL);
  latency_summary("faults, same region", fs[0].lat, fs[0].n);
  latency_summary("faults, other mapping", fs[1].lat, fs[1].n);
  latency_summary("advice flips", flip, flips);
  smaps(r + head, r + head + tail, &rss, &huge);
  printf("  tail after: rss %.0f MiB huge %.0f MiB, maps lines %d\n", rss / 1024.0, huge / 1024.0, maps_lines());
}

static void maps_of(const char *what, char *lo, char *hi) {
  FILE *f = fopen("/proc/self/maps", "r");
  char line[512];
  int n = 0;
  while (fgets(line, sizeof line, f)) {
    unsigned long a, b;
    if (sscanf(line, "%lx-%lx", &a, &b) == 2 && a < (unsigned long)hi && b > (unsigned long)lo) n++;
  }
  fclose(f);
  printf("  %-60s %d mappings\n", what, n);
}

static void exp_maps(void) {
  // A reservation like the allocator's: address space without access, regions made
  // accessible as they are used, each region starting advised against huge pages.
  size_t region = 256UL << 20, regions = 4;
  char *res = map_units(region * regions / UNIT);
  must(mprotect(res, region * regions, PROT_NONE), "reserve");
  for (size_t i = 0; i < regions; i++) {
    must(mprotect(res + i * region, region / 2, PROT_READ | PROT_WRITE), "commit");
    must(madvise(res + i * region, region, MADV_NOHUGEPAGE), "avoid");
  }
  char *e = res + region * regions;
  maps_of("4 regions, each half committed, advised against", res, e);
  for (size_t i = 0; i < regions; i++)
    must(madvise(res + i * region + (16UL << 20), region - (16UL << 20), MADV_HUGEPAGE), "prefer");
  maps_of("each advised for huge pages above 16 MiB", res, e);
  for (size_t i = 0; i < regions; i++)
    must(madvise(res + i * region + (64UL << 20), region - (64UL << 20), MADV_NOHUGEPAGE), "tail");
  maps_of("each with its tail above 64 MiB advised against again", res, e);
  must(madvise(res + (64UL << 20), region - (64UL << 20), MADV_HUGEPAGE), "back");
  maps_of("the first region's tail advised for huge pages again", res, e);
}

int main(int argc, char **argv) {
  setvbuf(stdout, NULL, _IOLBF, 0);
  cg_init();
  printf("cgroup %s, memory.max %ld\n", cgdir, cg("memory.max"));
  if (argc < 2) {
    fprintf(stderr, "usage: kernel partial|pressure|split|khuge|stall|maps args...\n");
    return 2;
  }
  const char *e = argv[1];
  if (!strcmp(e, "partial")) exp_partial(argc - 2, argv + 2);
  else if (!strcmp(e, "pressure")) exp_pressure(argc - 2, argv + 2);
  else if (!strcmp(e, "split")) exp_split(argc - 2, argv + 2);
  else if (!strcmp(e, "khuge")) exp_khuge(argc - 2, argv + 2);
  else if (!strcmp(e, "stall")) exp_stall(argc - 2, argv + 2);
  else if (!strcmp(e, "maps")) exp_maps();
  else {
    fprintf(stderr, "unknown experiment %s\n", e);
    return 2;
  }
  return 0;
}
