// dlopen and dlclose of a library that allocates, from several threads, with blocks crossing
// between the program and the library in both directions and outliving the library.
//
// usage: dl path/to/libdlib.so [rounds]
#include <dlfcn.h>
#include <pthread.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

typedef char *(*make_fn)(size_t);
typedef void (*take_fn)(char *);
typedef long (*tls_fn)(int);
typedef void (*release_fn)(void);

static const char *path;
static int rounds;

static void *use_library(void *arg) {
    long me = (long)arg;
    long sum = 0;
    for (int r = 0; r < rounds; r++) {
        void *h = dlopen(path, RTLD_NOW | RTLD_LOCAL);
        if (!h) {
            fprintf(stderr, "dlopen: %s\n", dlerror());
            exit(2);
        }
        make_fn make = (make_fn)dlsym(h, "dlib_make");
        take_fn take = (take_fn)dlsym(h, "dlib_take");
        tls_fn tls = (tls_fn)dlsym(h, "dlib_tls");
        release_fn release = (release_fn)dlsym(h, "dlib_tls_release");
        char *from_lib = make(100 + (size_t)r);
        char *to_lib = malloc(200);
        take(to_lib);
        sum += tls(r % 7);
        release();
        dlclose(h);
        // The library is gone; its block is still the program's.
        for (int i = 0; i < 100 + r; i++)
            if (from_lib[i] != 'D') exit(3);
        free(from_lib);
        // dlerror's message is malloc'd by the loader and freed by the next call.
        if (dlopen("/nonexistent/libnothing.so", RTLD_NOW) == NULL) (void)dlerror();
    }
    return (void *)(sum + me * 0);
}

int main(int argc, char **argv) {
    if (argc < 2) return 1;
    path = argv[1];
    rounds = argc > 2 ? atoi(argv[2]) : 200;
    pthread_t t[8];
    long total = 0;
    for (long i = 0; i < 8; i++) pthread_create(&t[i], NULL, use_library, (void *)i);
    for (int i = 0; i < 8; i++) {
        void *r;
        pthread_join(t[i], &r);
        total += (long)r;
    }
    printf("dlopen rounds %d checksum %ld\n", rounds, total);
    return 0;
}
