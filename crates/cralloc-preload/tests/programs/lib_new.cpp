// A shared library that replaces the global operators for whatever links it, with a header
// in front of each block: what a tracking or tagging allocator library does.
#include <cstdlib>
#include <new>

void *operator new(std::size_t n) {
    auto *h = static_cast<std::size_t *>(std::malloc(n + 16));
    if (!h) throw std::bad_alloc();
    h[0] = n;
    return h + 2;
}
void operator delete(void *p) noexcept {
    if (p) std::free(static_cast<std::size_t *>(p) - 2);
}
