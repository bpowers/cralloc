// A C++ program that brings its own malloc, as a program does that links the C half of
// another allocator, and uses the C++ runtime's operators. The program's malloc comes before
// any preloaded one, so the C++ runtime's throwing new, which calls malloc, gets its blocks
// from the program. A preloaded allocator must then serve both new and delete, or neither.
#include <cstdio>
#include <cstdlib>
#include <string>
#include <vector>

extern "C" {
void *__libc_malloc(size_t);
void __libc_free(void *);
void *__libc_calloc(size_t, size_t);
void *__libc_realloc(void *, size_t);
void *__libc_memalign(size_t, size_t);
void *malloc(size_t n) { return __libc_malloc(n); }
void free(void *p) { __libc_free(p); }
void *calloc(size_t n, size_t size) { return __libc_calloc(n, size); }
void *realloc(void *p, size_t n) { return __libc_realloc(p, n); }
void *aligned_alloc(size_t align, size_t n) { return __libc_memalign(align, n); }
}

int main() {
    std::vector<std::string> strings;
    for (int i = 0; i < 1000; i++) strings.push_back(std::string(i % 200, 'x'));
    size_t total = 0;
    for (auto &s : strings) total += s.size();
    std::printf("%zu\n", total);
    return 0;
}
