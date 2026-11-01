// Forks from many threads at once while every thread allocates. Each child allocates, frees
// blocks its parent made (inherited copies), starts a thread of its own, checks contents, and
// reports through its exit status. Prints counts that do not depend on scheduling.
//
// usage: forks [threads forks_per_thread]
#include <pthread.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/wait.h>
#include <unistd.h>

static int forks_per_thread;
static volatile int stop;

static uint64_t next(uint64_t *s) {
    *s ^= *s << 13;
    *s ^= *s >> 7;
    *s ^= *s << 17;
    return *s;
}

static void *churn(void *arg) {
    uint64_t s = (uint64_t)(uintptr_t)arg | 1;
    void *keep[64] = {0};
    while (!stop) {
        size_t i = next(&s) % 64;
        free(keep[i]);
        keep[i] = malloc(next(&s) % 4096 + 1);
    }
    for (int i = 0; i < 64; i++) free(keep[i]);
    return NULL;
}

static void *child_thread(void *arg) {
    char **blocks = arg;
    for (int i = 0; i < 100; i++) {
        blocks[i] = malloc(100 + (size_t)i);
        memset(blocks[i], i, 100 + (size_t)i);
    }
    return NULL;
}

// The child: returns 0 if everything it saw was right.
static int child(unsigned char **inherited, int n) {
    for (int i = 0; i < n; i++) {
        for (size_t j = 0; j < 200; j++)
            if (inherited[i][j] != (unsigned char)(i + 7)) return 2;
        free(inherited[i]);
    }
    char *blocks[100];
    pthread_t t;
    if (pthread_create(&t, NULL, child_thread, blocks) != 0) return 3;
    pthread_join(t, NULL);
    for (int i = 0; i < 100; i++) {
        for (int j = 0; j < 100 + i; j++)
            if (blocks[i][j] != (char)i) return 4;
        free(blocks[i]);
    }
    uint64_t s = 12345;
    for (int i = 0; i < 20000; i++) {
        size_t size = next(&s) % 70000 + 1;
        unsigned char *p = malloc(size);
        if (!p) return 5;
        p[0] = 1;
        p[size - 1] = 2;
        free(p);
    }
    return 0;
}

static void *forker(void *arg) {
    long me = (long)arg;
    long ok = 0;
    for (int f = 0; f < forks_per_thread; f++) {
        unsigned char *inherited[16];
        for (int i = 0; i < 16; i++) {
            inherited[i] = malloc(200 + (size_t)(me * 16 + i));
            memset(inherited[i], i + 7, 200);
        }
        pid_t pid = fork();
        if (pid == 0) _exit(child(inherited, 16));
        int status = 0;
        if (pid > 0 && waitpid(pid, &status, 0) == pid && WIFEXITED(status) &&
            WEXITSTATUS(status) == 0)
            ok++;
        else
            fprintf(stderr, "child of thread %ld failed: status %#x\n", me, status);
        for (int i = 0; i < 16; i++) free(inherited[i]);
    }
    return (void *)ok;
}

int main(int argc, char **argv) {
    int threads = argc > 1 ? atoi(argv[1]) : 8;
    forks_per_thread = argc > 2 ? atoi(argv[2]) : 40;
    pthread_t churners[4], forkers[64];
    for (long i = 0; i < 4; i++) pthread_create(&churners[i], NULL, churn, (void *)(i + 1));
    long ok = 0;
    for (long i = 0; i < threads; i++) pthread_create(&forkers[i], NULL, forker, (void *)i);
    for (int i = 0; i < threads; i++) {
        void *r;
        pthread_join(forkers[i], &r);
        ok += (long)r;
    }
    stop = 1;
    for (int i = 0; i < 4; i++) pthread_join(churners[i], NULL);
    printf("forks %ld of %ld children ok\n", ok, (long)threads * forks_per_thread);
    return ok != (long)threads * forks_per_thread;
}
