// Preloaded after the allocator under test. At exit it writes one line of JSON to the file
// named by BENCH_PROBE_OUT saying which allocator served the process and, for tcmalloc,
// whether its per-CPU caches were active. tcmalloc falls back to per-thread caches without
// saying so (when glibc already owns the thread's rseq area), so a run must check.
//
// It also says which object serves malloc ("library": the canonical path of the shared
// library, or "executable"), so a run can check that the library it preloaded is the one in
// use, and records the process's peak resident set (VmHWM), which the parent cannot get
// exactly: getrusage on a forked child counts the parent's memory at the fork.
#define _GNU_SOURCE
#include <dlfcn.h>
#include <limits.h>
#include <link.h>
#include <stdbool.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

// glibc 2.35 and later export the size of the rseq area they registered; 0 means glibc did
// not register one (for example under GLIBC_TUNABLES=glibc.pthread.rseq=0).
extern const unsigned int __rseq_size __attribute__((weak));

static void json_string(FILE *f, const char *s) {
  fputc('"', f);
  for (; *s != '\0'; s++) {
    unsigned char c = (unsigned char)*s;
    if (c == '"' || c == '\\') fprintf(f, "\\%c", c);
    else if (c < 0x20) fprintf(f, "\\u%04x", c);
    else fputc(c, f);
  }
  fputc('"', f);
}

__attribute__((destructor)) static void bench_probe_report(void) {
  const char *path = getenv("BENCH_PROBE_OUT");
  if (path == NULL || path[0] == '\0') return;
  FILE *f = fopen(path, "w");
  if (f == NULL) return;

  bool (*tc_percpu)(void) =
      (bool (*)(void))dlsym(RTLD_DEFAULT, "MallocExtension_Internal_GetPerCpuCachesActive");
  int (*mi_version)(void) = (int (*)(void))dlsym(RTLD_DEFAULT, "mi_version");
  int (*je_mallctl)(const char *, void *, size_t *, void *, size_t) =
      (int (*)(const char *, void *, size_t *, void *, size_t))dlsym(RTLD_DEFAULT, "mallctl");

  const char *allocator = "glibc";
  if (tc_percpu != NULL) allocator = "tcmalloc";
  else if (mi_version != NULL) allocator = "mimalloc";
  else if (je_mallctl != NULL) allocator = "jemalloc";

  const char *mode = "";
  if (tc_percpu != NULL) mode = tc_percpu() ? "per-cpu" : "per-thread";

  char library[PATH_MAX] = "unknown";
  Dl_info info;
  struct link_map *map = NULL;
  void *malloc_fn = dlsym(RTLD_DEFAULT, "malloc");
  if (malloc_fn != NULL && dladdr1(malloc_fn, &info, (void **)&map, RTLD_DL_LINKMAP) != 0) {
    // The main program's link map has an empty name.
    if (map != NULL && map->l_name[0] == '\0') {
      strcpy(library, "executable");
    } else if (info.dli_fname != NULL && realpath(info.dli_fname, library) == NULL) {
      snprintf(library, sizeof library, "%s", info.dli_fname);
    }
  }

  char version[64] = "";
  if (strcmp(allocator, "jemalloc") == 0) {
    const char *v = NULL;
    size_t len = sizeof v;
    if (je_mallctl("version", &v, &len, NULL, 0) == 0 && v != NULL) snprintf(version, sizeof version, "%s", v);
  } else if (mi_version != NULL) {
    snprintf(version, sizeof version, "%d", mi_version());
  }

  unsigned long long hwm_kb = 0;
  FILE *st = fopen("/proc/self/status", "r");
  if (st != NULL) {
    char line[256];
    while (fgets(line, sizeof line, st) != NULL) {
      if (sscanf(line, "VmHWM: %llu kB", &hwm_kb) == 1) break;
    }
    fclose(st);
  }

  fprintf(f, "{\"allocator\":\"%s\",\"mode\":\"%s\",\"library\":", allocator, mode);
  json_string(f, library);
  fputs(",\"version\":", f);
  json_string(f, version);
  fprintf(f, ",\"glibc_rseq_size\":%u,\"vm_hwm\":%llu}\n", &__rseq_size != NULL ? __rseq_size : 0u,
          hwm_kb * 1024);
  fclose(f);
}
