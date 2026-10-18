// A program that replaces operator new(size_t) and operator delete(void*), the two the C++
// standard says are enough: the default of every other form forwards to them. The
// replacement keeps a header in front of each block, as counting and tagging allocators do,
// so its blocks are not block starts of whatever serves malloc.
//
// argv[1] picks the pair: "nothrow" (nothrow new, plain delete), "sized" (plain new, sized
// delete, through a container), "array" (new[], delete[]).
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <new>
#include <string>
#include <vector>

static long live;

void *operator new(std::size_t n) {
    auto *header = static_cast<std::size_t *>(std::malloc(n + 16));
    if (!header) throw std::bad_alloc();
    header[0] = n;
    ++live;
    return header + 2;
}

void operator delete(void *p) noexcept {
    if (!p) return;
    --live;
    std::free(static_cast<std::size_t *>(p) - 2);
}

struct Record {
    long a, b, c;
};

// Keeps the compiler from pairing a new with its delete and removing both.
template <typename T> static T *opaque(T *p) {
    static T *volatile sink;
    sink = p;
    return sink;
}

int main(int argc, char **argv) {
    const char *mode = argc > 1 ? argv[1] : "sized";
    if (!std::strcmp(mode, "nothrow")) {
        Record *r = opaque(new (std::nothrow) Record);
        ::operator delete(r);
    } else if (!std::strcmp(mode, "array")) {
        int *a = opaque(new int[100]);
        delete[] a;
    } else {
        std::vector<std::string> strings(100, std::string(100, 'x'));
        opaque(strings.data());
    }
    std::printf("%s: live %ld\n", mode, live);
    return 0;
}
