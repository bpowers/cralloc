// Does MADV_COLLAPSE make huge pages of a range advised MADV_NOHUGEPAGE, as asmalloc's regions
// start? And of one touched sparsely? (docs/research/whole-block-huge.md)
//
//   cc -O2 -o collapse collapse.c && scripts/memlimit -m 1G -- ./collapse
//
// For each case: maps 16 MiB aligned to 2 MiB, gives the advice, touches one page in `every`,
// asks MADV_COLLAPSE of the whole range, and prints its result and the range's
// AnonHugePages and Rss from /proc/self/smaps.
#define _GNU_SOURCE
#include <errno.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <time.h>

#ifndef MADV_COLLAPSE
#define MADV_COLLAPSE 25
#endif

static void range_stats(void *p, long *rss, long *huge) {
  FILE *f = fopen("/proc/self/smaps", "r");
  char line[512];
  int in = 0;
  *rss = *huge = -1;
  unsigned long want = (unsigned long)p;
  while (fgets(line, sizeof line, f)) {
    unsigned long a, b;
    char perm[8];
    if (sscanf(line, "%lx-%lx %7s", &a, &b, perm) == 3) {
      in = a <= want && want < b;
      continue;
    }
    if (!in) continue;
    if (!strncmp(line, "Rss:", 4)) *rss = atol(line + 4);
    if (!strncmp(line, "AnonHugePages:", 14)) *huge = atol(line + 14);
  }
  fclose(f);
}

static void run(const char *name, int advice, int every) {
  size_t len = 16 << 20, huge = 2 << 20;
  char *raw = mmap(NULL, len + huge, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
  char *p = (char *)(((uintptr_t)raw + huge - 1) & ~(huge - 1));
  if (advice >= 0) madvise(p, len, advice);
  for (size_t i = 0; i < len; i += 4096 * (size_t)every) p[i] = 1;
  long rss0, huge0, rss1, huge1;
  range_stats(p, &rss0, &huge0);
  struct timespec t0, t1;
  clock_gettime(CLOCK_MONOTONIC, &t0);
  int r = madvise(p, len, MADV_COLLAPSE);
  int e = r ? errno : 0;
  clock_gettime(CLOCK_MONOTONIC, &t1);
  range_stats(p, &rss1, &huge1);
  double us = (t1.tv_sec - t0.tv_sec) * 1e6 + (t1.tv_nsec - t0.tv_nsec) / 1e3;
  printf("%-28s every=%-4d before rss=%6ld KiB huge=%6ld | collapse rc=%d %-8s %8.0f us | after rss=%6ld huge=%6ld\n",
         name, every, rss0, huge0, r, e ? strerror(e) : "ok", us, rss1, huge1);
  munmap(raw, len + huge);
}

int main(void) {
  int everies[] = {1, 64, 512};
  for (int i = 0; i < 3; i++) {
    run("MADV_NOHUGEPAGE", MADV_NOHUGEPAGE, everies[i]);
    run("no advice", -1, everies[i]);
    run("MADV_HUGEPAGE", MADV_HUGEPAGE, everies[i]);
  }
  // A range advised against, then the advice taken back to MADV_HUGEPAGE: the grown path's
  // sequence, before the collapse.
  {
    size_t len = 16 << 20, huge = 2 << 20;
    char *raw = mmap(NULL, len + huge, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    char *p = (char *)(((uintptr_t)raw + huge - 1) & ~(huge - 1));
    madvise(p, len, MADV_NOHUGEPAGE);
    for (size_t i = 0; i < len; i += 4096) p[i] = 1;
    madvise(p, len, MADV_HUGEPAGE);
    long rss0, huge0, rss1, huge1;
    range_stats(p, &rss0, &huge0);
    int r = madvise(p, len, MADV_COLLAPSE);
    range_stats(p, &rss1, &huge1);
    printf("%-28s every=1    before rss=%6ld KiB huge=%6ld | collapse rc=%d %s | after rss=%6ld huge=%6ld\n",
           "NOHUGEPAGE then HUGEPAGE", rss0, huge0, r, r ? strerror(errno) : "ok", rss1, huge1);
  }
  return 0;
}
