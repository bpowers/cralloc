// What MADV_DONTNEED on part of a transparent huge page returns, and when: the pages mapped
// (RSS), the huge pages mapped, and what the kernel charges the process's cgroup, before and
// after half of each huge page of a 64 MiB block is purged. The allocator is not involved; run
// it in a cgroup of its own (scripts/memlimit). On Linux 7.2.9 the mapped pages halve at once
// and the charge for the huge pages does not move: the kernel keeps a huge page whole until
// all of it is unmapped or memory pressure splits it. (So a shrink in place in a slot that
// inherited huge pages is not trimmed to them.)
// usage: scripts/memlimit -m 1G -- thp_partial
#define _GNU_SOURCE
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <unistd.h>

static long statm_rss_kib(void) {
    long size, rss;
    FILE *f = fopen("/proc/self/statm", "r");
    if (!f || fscanf(f, "%ld %ld", &size, &rss) != 2) return -1;
    fclose(f);
    return rss * 4;
}

static long field_kib(const char *path, const char *key) {
    char line[256];
    long v = -1;
    FILE *f = fopen(path, "r");
    if (!f) return -1;
    while (fgets(line, sizeof line, f))
        if (!strncmp(line, key, strlen(key))) { sscanf(line + strlen(key), "%ld", &v); break; }
    fclose(f);
    return v;
}

static long cgroup_kib(void) {
    char line[512], path[600];
    FILE *f = fopen("/proc/self/cgroup", "r");
    if (!f || !fgets(line, sizeof line, f)) return -1;
    fclose(f);
    char *p = strchr(line, ':');
    p = p ? strchr(p + 1, ':') : 0;
    if (!p) return -1;
    p[strcspn(p, "\n")] = 0;
    snprintf(path, sizeof path, "/sys/fs/cgroup%s/memory.current", p + 1);
    f = fopen(path, "r");
    long v = -1;
    if (f) { if (fscanf(f, "%ld", &v) == 1) v /= 1024; fclose(f); }
    return v;
}

static void show(const char *what) {
    printf("%-34s rss %7ld KiB  AnonHuge %7ld KiB  cgroup %7ld KiB\n", what, statm_rss_kib(),
           field_kib("/proc/self/smaps_rollup", "AnonHugePages:"), cgroup_kib());
}

int main(void) {
    size_t huge = 2 << 20, n = 32 * huge;
    char *raw = mmap(0, n + huge, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    char *p = (char *)(((size_t)raw + huge - 1) & ~(huge - 1));
    show("start");
    madvise(p, n, MADV_HUGEPAGE);
    for (size_t i = 0; i < n; i += 4096) p[i] = 1;
    show("64 MiB written under MADV_HUGEPAGE");
    madvise(p, n, MADV_NOHUGEPAGE);
    // Half of every huge page.
    for (size_t i = 0; i < n; i += huge) madvise(p + i + huge / 2, huge / 2, MADV_DONTNEED);
    show("half of each huge page DONTNEED");
    sleep(2);
    show("2 s later");
    madvise(p, n, MADV_DONTNEED);
    show("all DONTNEED");
    return 0;
}
