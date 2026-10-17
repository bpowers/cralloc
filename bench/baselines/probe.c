// Preloaded after the allocator under test. At exit it writes one line of JSON to the file
// named by BENCH_PROBE_OUT saying which allocator served the process and, for tcmalloc,
// whether its per-CPU caches were active. tcmalloc falls back to per-thread caches without
// saying so (when glibc already owns the thread's rseq area), so a run must check.
//
// It also records the process's peak resident set (VmHWM), which the parent cannot get
// exactly: getrusage on a forked child counts the parent's memory at the fork.
#define _GNU_SOURCE
#include <dlfcn.h>
#include <stdbool.h>
#include <stdio.h>
#include <stdlib.h>

// glibc 2.35 and later export the size of the rseq area they registered; 0 means glibc did
// not register one (for example under GLIBC_TUNABLES=glibc.pthread.rseq=0).
extern const unsigned int __rseq_size __attribute__((weak));

__attribute__((destructor)) static void bench_probe_report(void) {
  const char *path = getenv("BENCH_PROBE_OUT");
  if (path == NULL || path[0] == '\0') return;
  FILE *f = fopen(path, "w");
  if (f == NULL) return;

  bool (*tc_percpu)(void) =
      (bool (*)(void))dlsym(RTLD_DEFAULT, "MallocExtension_Internal_GetPerCpuCachesActive");
  int (*mi_version)(void) = (int (*)(void))dlsym(RTLD_DEFAULT, "mi_version");
  void *je_mallctl = dlsym(RTLD_DEFAULT, "mallctl");

  const char *allocator = "glibc";
  if (tc_percpu != NULL) allocator = "tcmalloc";
  else if (mi_version != NULL) allocator = "mimalloc";
  else if (je_mallctl != NULL) allocator = "jemalloc";

  const char *mode = "";
  if (tc_percpu != NULL) mode = tc_percpu() ? "per-cpu" : "per-thread";

  unsigned long long hwm_kb = 0;
  FILE *st = fopen("/proc/self/status", "r");
  if (st != NULL) {
    char line[256];
    while (fgets(line, sizeof line, st) != NULL) {
      if (sscanf(line, "VmHWM: %llu kB", &hwm_kb) == 1) break;
    }
    fclose(st);
  }

  fprintf(f, "{\"allocator\":\"%s\",\"mode\":\"%s\",\"glibc_rseq_size\":%u,\"vm_hwm\":%llu}\n",
          allocator, mode, &__rseq_size != NULL ? __rseq_size : 0u, hwm_kb * 1024);
  fclose(f);
}
