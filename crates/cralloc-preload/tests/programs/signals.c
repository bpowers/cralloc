// Allocates and frees in a signal handler that interrupts threads that are themselves
// allocating. C does not allow malloc in a handler; cralloc does (design: "Signals"), so this
// runs only under cralloc.
#include <pthread.h>
#include <signal.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <unistd.h>

static volatile sig_atomic_t handled;
static volatile int stop;

static void handler(int sig) {
    (void)sig;
    void *p[8];
    for (int i = 0; i < 8; i++) {
        p[i] = malloc((size_t)(i + 1) * 24);
        memset(p[i], 0x5a, (size_t)(i + 1) * 24);
    }
    for (int i = 0; i < 8; i++) free(p[i]);
    handled++;
}

static void *churn(void *arg) {
    uint64_t s = (uint64_t)(uintptr_t)arg | 1;
    void *keep[64] = {0};
    while (!stop) {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        size_t i = s % 64;
        free(keep[i]);
        keep[i] = malloc(s % 200 + 1);
    }
    for (int i = 0; i < 64; i++) free(keep[i]);
    return NULL;
}

int main(void) {
    struct sigaction sa;
    memset(&sa, 0, sizeof sa);
    sa.sa_handler = handler;
    sa.sa_flags = SA_RESTART;
    sigaction(SIGUSR1, &sa, NULL);
    pthread_t t[4];
    for (long i = 0; i < 4; i++) pthread_create(&t[i], NULL, churn, (void *)(i + 1));
    for (int n = 0; n < 20000; n++) pthread_kill(t[n % 4], SIGUSR1);
    struct timespec pause = {0, 50 * 1000 * 1000};
    nanosleep(&pause, NULL);
    stop = 1;
    for (int i = 0; i < 4; i++) pthread_join(t[i], NULL);
    printf("signals handled: %s\n", handled > 0 ? "some" : "none");
    return 0;
}
