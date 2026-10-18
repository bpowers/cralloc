// Loads a library that has asmalloc inside it (argv[1]), calls it, unloads it and keeps
// running, a few times over. After the unload the calling thread's rseq_cs still names a
// descriptor of the library's routines, in memory that is no longer mapped; the kernel reads
// it at the thread's next preemption or signal and kills the process with SIGSEGV.
#define _GNU_SOURCE
#include <dlfcn.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

static unsigned long address_space_gib(void) {
    FILE *f = fopen("/proc/self/status", "r");
    char line[256];
    unsigned long kib = 0;
    while (f && fgets(line, sizeof line, f)) {
        if (!strncmp(line, "VmSize:", 7)) kib = strtoul(line + 7, 0, 10);
    }
    if (f) fclose(f);
    return kib >> 20;
}

int main(int argc, char **argv) {
    int rounds = argc > 2 ? atoi(argv[2]) : 3;
    for (int i = 0; i < rounds; i++) {
        void *lib = dlopen(argv[1], RTLD_NOW | RTLD_LOCAL);
        if (!lib) {
            fprintf(stderr, "dlopen: %s\n", dlerror());
            return 2;
        }
        size_t (*work)(size_t) = (size_t (*)(size_t))dlsym(lib, "plugin_work");
        size_t sum = work(1000);
        if (dlclose(lib)) {
            fprintf(stderr, "dlclose: %s\n", dlerror());
            return 2;
        }
        // Long enough to be preempted.
        struct timespec start, now;
        clock_gettime(CLOCK_MONOTONIC, &start);
        do {
            clock_gettime(CLOCK_MONOTONIC, &now);
        } while ((now.tv_sec - start.tv_sec) * 1000 + (now.tv_nsec - start.tv_nsec) / 1000000 < 30);
        printf("round %d sum %zu address space %lu GiB\n", i, sum, address_space_gib());
        fflush(stdout);
    }
    puts("done");
    return 0;
}
