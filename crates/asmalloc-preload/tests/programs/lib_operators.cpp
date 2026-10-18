// A shared library that defines the C++ allocation operators, as Boehm GC's libgccpp does and as
// engines that ship their allocator's operators in a library do. Its blocks come from an arena of
// its own, so they are no other allocator's, and its delete checks that each block is one of its.
// The loader looks it up after the executable and any preloaded library, so a preloaded library
// that defined some of these operators would take its deletes and be handed its blocks.
//
// Built with -DPROGRAM, the same file is the program: it uses new and delete in the usual ways
// and prints how many of the library's blocks are still live.
#include <cstddef>
#include <cstdio>
#include <cstdlib>
#include <new>

#ifndef PROGRAM

alignas(64) static unsigned char arena[64 << 20];
static std::size_t used;
long operators_live;

static void *take(std::size_t n) {
    std::size_t at = (used + 15) & ~std::size_t(15);
    if (at + n > sizeof arena) throw std::bad_alloc();
    used = at + n;
    ++operators_live;
    return arena + at;
}

static void give(void *p) {
    if (!p) return;
    if (static_cast<unsigned char *>(p) < arena || static_cast<unsigned char *>(p) >= arena + sizeof arena) {
        std::fputs("lib_operators: delete of a block that is not mine\n", stderr);
        std::abort();
    }
    --operators_live;
}

void *operator new(std::size_t n) { return take(n); }
void *operator new[](std::size_t n) { return take(n); }
void *operator new(std::size_t n, const std::nothrow_t &) noexcept {
    try { return take(n); } catch (...) { return nullptr; }
}
void *operator new[](std::size_t n, const std::nothrow_t &) noexcept {
    try { return take(n); } catch (...) { return nullptr; }
}
void operator delete(void *p) noexcept { give(p); }
void operator delete[](void *p) noexcept { give(p); }
void operator delete(void *p, std::size_t) noexcept { give(p); }
void operator delete[](void *p, std::size_t) noexcept { give(p); }
void operator delete(void *p, const std::nothrow_t &) noexcept { give(p); }
void operator delete[](void *p, const std::nothrow_t &) noexcept { give(p); }

#else

#include <string>
#include <vector>

extern long operators_live;

template <typename T> static T *opaque(T *p) {
    static T *volatile sink;
    sink = p;
    return sink;
}

int main() {
    {
        std::vector<std::string> strings;
        for (int i = 0; i < 2000; i++) strings.push_back(std::string(i % 300, 'x'));
        int *a = opaque(new int[100]);
        delete[] a;
        long *b = opaque(new (std::nothrow) long);
        delete b;
    }
    std::printf("live %ld\n", operators_live);
    return 0;
}

#endif
