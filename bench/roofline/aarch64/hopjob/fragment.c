// Uses up the kernel's free blocks of a huge page and keeps them from coming back, for what a shortage
// costs: writes small pages until no free block is left or BUDGET GiB are written, then gives back
// all but one page in eight MiB, and holds those until it is signalled.
//
//   cc -O2 fragment.c -o fragment ; scripts/memlimit -m 13G -- ./fragment BUDGET_GIB
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <unistd.h>

// The free blocks of the largest order, the last number of the zone's line.
static long free_blocks(void) {
    FILE *f = fopen("/proc/buddyinfo", "r");
    char line[512];
    long last = -1, total = 0;
    while (f && fgets(line, sizeof line, f)) {
        char *end = line + strlen(line);
        while (end > line && (end[-1] == '\n' || end[-1] == ' ')) *--end = 0;
        char *space = strrchr(line, ' ');
        if (space) total += atol(space + 1);
        last = total;
    }
    if (f) fclose(f);
    return last;
}

int main(int argc, char **argv) {
    size_t budget = (size_t)(argc > 1 ? atol(argv[1]) : 8) << 30;
    size_t page = (size_t)sysconf(_SC_PAGESIZE), step = (size_t)256 << 20, keep = (size_t)8 << 20;
    char *base = mmap(NULL, budget, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (base == MAP_FAILED) return perror("mmap"), 1;
    madvise(base, budget, MADV_NOHUGEPAGE);
    printf("pid %d\n", (int)getpid());
    printf("free blocks at the start: %ld\n", free_blocks());
    size_t used = 0;
    while (used < budget && free_blocks() > 0) {
        for (size_t at = 0; at < step; at += page) base[used + at] = 1;
        used += step;
    }
    printf("wrote %zu MiB, free blocks: %ld\n", used >> 20, free_blocks());
    for (size_t at = 0; at + keep <= used; at += keep) madvise(base + at + page, keep - page, MADV_DONTNEED);
    printf("gave back all but one page in %zu MiB, free blocks: %ld\n", keep >> 20, free_blocks());
    fflush(stdout);
    pause();
    return 0;
}
