// A library that allocates when loaded and unloaded, has thread-local storage (which the
// dynamic loader allocates with malloc in every thread that touches it), and hands blocks to
// and from the program that loads it.
#include <stdlib.h>
#include <string.h>

static char *made_at_load;
static __thread char tls_buffer[4096];
static __thread char *tls_block;

__attribute__((constructor)) static void loaded(void) {
    made_at_load = malloc(1000);
    memset(made_at_load, 'L', 1000);
}

__attribute__((destructor)) static void unloaded(void) {
    for (int i = 0; i < 1000; i++)
        if (made_at_load[i] != 'L') abort();
    free(made_at_load);
}

// A block the caller frees.
char *dlib_make(size_t size) {
    char *p = malloc(size);
    memset(p, 'D', size);
    return p;
}

// Frees a block the caller made.
void dlib_take(char *p) { free(p); }

// Touches this thread's TLS, keeps a block in it, and returns a checksum.
long dlib_tls(int round) {
    memset(tls_buffer, round, sizeof tls_buffer);
    free(tls_block);
    tls_block = malloc(64 + (size_t)round);
    long sum = 0;
    for (size_t i = 0; i < sizeof tls_buffer; i++) sum += tls_buffer[i];
    return sum;
}

void dlib_tls_release(void) {
    free(tls_block);
    tls_block = NULL;
}
