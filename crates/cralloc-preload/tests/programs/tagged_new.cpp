// Replaces all four basic operators and tags each block as scalar or array, as a checker for
// new/delete[] mismatches does. Every other form's standard default forwards to the matching
// basic form: the array forms to operator new[] and operator delete[].
#include <cstdio>
#include <cstdlib>
#include <new>

static int mismatches;
static void *tagged(std::size_t n, std::size_t tag) {
    auto *h = static_cast<std::size_t *>(std::malloc(n + 16));
    if (!h) throw std::bad_alloc();
    h[0] = tag;
    return h + 2;
}
static void untag(void *p, std::size_t tag) {
    if (!p) return;
    auto *h = static_cast<std::size_t *>(p) - 2;
    if (h[0] != tag) mismatches++;
    std::free(h);
}
void *operator new(std::size_t n) { return tagged(n, 1); }
void *operator new[](std::size_t n) { return tagged(n, 2); }
void operator delete(void *p) noexcept { untag(p, 1); }
void operator delete[](void *p) noexcept { untag(p, 2); }

struct Dtor {
    long x;
    ~Dtor() { x = 0; }
};
template <typename T> static T *opaque(T *p) {
    static T *volatile sink;
    sink = p;
    return sink;
}

int main() {
    delete[] opaque(new Dtor[3]);                      // sized delete[]: _ZdaPvm
    int *n = opaque(new (std::nothrow) int[10]);       // nothrow new[]: _ZnamRKSt9nothrow_t
    delete[] n;                                        // the program's delete[]
    ::operator delete[](opaque(new int[4]), std::nothrow); // nothrow delete[]
    std::printf("mismatches %d\n", mismatches);
    return 0;
}
