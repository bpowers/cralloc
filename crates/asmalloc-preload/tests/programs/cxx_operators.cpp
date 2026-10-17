// Conformance cases for the replaceable C++ allocation operators, every form called explicitly:
// alignment, zero size, array forms, sized deletes with right and wrong sizes, the new-handler
// loop, nothrow failure. Each case prints "ok <name>" or "FAIL <name>: <why>", and the program
// exits non-zero if any failed. Every case passes with libstdc++'s own operators too.
//
// usage: cxx_operators            the cases
//        cxx_operators throw      a throwing new that fails, caught as std::bad_alloc
//        cxx_operators throwing-handler
//                                 a nothrow new whose new-handler throws std::bad_alloc
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <malloc.h>
#include <memory>
#include <new>
#include <string>
#include <vector>

static int failures;

static void report(bool ok, const char *name, const std::string &why = "") {
    if (ok) {
        std::printf("ok %s\n", name);
    } else {
        std::printf("FAIL %s: %s\n", name, why.c_str());
        failures++;
    }
}

// A size the compiler cannot see, too large for any heap.
static volatile std::size_t huge = std::size_t(1) << 62;

static bool aligned(void *p, std::size_t align) {
    return reinterpret_cast<std::uintptr_t>(p) % align == 0;
}

// The alignment C++ promises a plain new of `size` bytes: the default new alignment for blocks
// that large, else the largest power of two not above the size.
static std::size_t natural(std::size_t size) {
    std::size_t a = 1;
    while (a * 2 <= size && a * 2 <= __STDCPP_DEFAULT_NEW_ALIGNMENT__) a *= 2;
    return a;
}

static bool usable(void *p, std::size_t size, const char *how, std::string &why) {
    if (p == nullptr) {
        why = std::string(how) + " returned null";
        return false;
    }
    std::memset(p, 0xA5, size);
    for (std::size_t i = 0; i < size; i++)
        if (static_cast<unsigned char *>(p)[i] != 0xA5) {
            why = std::string(how) + " does not hold what was written";
            return false;
        }
    if (malloc_usable_size(p) < size) {
        why = std::string(how) + " is smaller than asked";
        return false;
    }
    return true;
}

static void plain_forms() {
    const std::size_t sizes[] = {1, 7, 8, 15, 16, 24, 100, 4096, 40000, 1 << 20};
    std::string why;
    bool ok = true;
    for (std::size_t n : sizes) {
        void *a = ::operator new(n);
        void *b = ::operator new[](n);
        void *c = ::operator new(n, std::nothrow);
        void *d = ::operator new[](n, std::nothrow);
        for (void *p : {a, b, c, d}) {
            ok = ok && usable(p, n, "new", why);
            if (ok && !aligned(p, natural(n))) {
                ok = false;
                why = "new(" + std::to_string(n) + ") is not " + std::to_string(natural(n)) +
                      "-aligned";
            }
        }
        ::operator delete(a);
        ::operator delete[](b);
        ::operator delete(c, std::nothrow);
        ::operator delete[](d, std::nothrow);
    }
    report(ok, "plain, array and nothrow new are usable and aligned", why);
}

static void aligned_forms() {
    const std::size_t aligns[] = {1, 8, 16, 32, 64, 4096, 1 << 16, 1 << 21};
    const std::size_t sizes[] = {1, 100, 5000, 3 << 20};
    std::string why;
    bool ok = true;
    for (std::size_t al : aligns) {
        for (std::size_t n : sizes) {
            std::align_val_t a{al};
            void *p[4] = {::operator new(n, a), ::operator new[](n, a),
                          ::operator new(n, a, std::nothrow), ::operator new[](n, a, std::nothrow)};
            for (void *q : p) {
                ok = ok && usable(q, n, "aligned new", why);
                if (ok && !aligned(q, al)) {
                    ok = false;
                    why = "new(" + std::to_string(n) + ", align " + std::to_string(al) +
                          ") is misaligned";
                }
            }
            ::operator delete(p[0], a);
            ::operator delete[](p[1], n, a);
            ::operator delete(p[2], a, std::nothrow);
            ::operator delete[](p[3], a, std::nothrow);
            void *s = ::operator new(n, a);
            ::operator delete(s, n, a);
            void *t = ::operator new[](n, a);
            ::operator delete[](t, a);
        }
    }
    report(ok, "aligned new of every form meets its alignment", why);
}

static void zero_size() {
    void *a = ::operator new(0);
    void *b = ::operator new(0);
    void *c = ::operator new[](0);
    void *d = ::operator new(0, std::align_val_t{64});
    void *e = ::operator new(0, std::nothrow);
    bool ok = a && b && c && d && e && a != b && a != c && b != c && aligned(d, 64);
    ::operator delete(a, std::size_t(0));
    ::operator delete(b);
    ::operator delete[](c, std::size_t(0));
    ::operator delete(d, std::size_t(0), std::align_val_t{64});
    ::operator delete(e);
    report(ok, "zero-size new gives distinct, non-null blocks", ok ? "" : "a null or repeated block");
}

struct Base {
    std::uint64_t a;
    ~Base() {}
};
struct Derived : Base {
    char tail[1000];
};

static void wrong_sizes() {
    // Live blocks with known contents; frees with wrong sizes between them must not disturb
    // them or the blocks allocated afterwards.
    std::vector<std::pair<unsigned char *, std::size_t>> live;
    for (int i = 0; i < 2000; i++) {
        std::size_t n = 1 + (i * 37) % 3000;
        auto *p = static_cast<unsigned char *>(::operator new(n));
        std::memset(p, i & 0xff, n);
        live.push_back({p, n});
        void *victim = ::operator new(1 + (i * 53) % 5000);
        switch (i % 5) {
        case 0: ::operator delete(victim, std::size_t(1)); break;
        case 1: ::operator delete(victim, std::size_t(1) << 30); break;
        case 2:
            ::operator delete(victim);
            victim = ::operator new[](1 + (i * 53) % 5000);
            ::operator delete[](victim, std::size_t(7));
            break;
        case 3: ::operator delete(victim, std::size_t(3), std::align_val_t{4096}); break;
        default: {
            // The classic: a derived object deleted through a base pointer without a virtual
            // destructor. The compiler passes sizeof(Base) to the sized delete.
            ::operator delete(victim);
            Base *b = new Derived;
            delete b;
        }
        }
    }
    bool ok = true;
    for (std::size_t i = 0; i < live.size(); i++) {
        for (std::size_t j = 0; j < live[i].second; j++)
            if (live[i].first[j] != (i & 0xff)) ok = false;
        ::operator delete(live[i].first, live[i].second);
    }
    report(ok, "sized deletes with wrong sizes free what unsized deletes would",
           ok ? "" : "a live block changed");
}

static void nothrow_failure() {
    void *a = ::operator new(huge, std::nothrow);
    void *b = ::operator new[](huge, std::nothrow);
    void *c = ::operator new(huge, std::align_val_t{64}, std::nothrow);
    void *d = ::operator new[](huge, std::align_val_t{4096}, std::nothrow);
    report(!a && !b && !c && !d, "a nothrow new that cannot be served returns null",
           "a huge nothrow new returned a block");
}

static int handler_calls;

static void counting_handler() {
    if (++handler_calls == 3) std::set_new_handler(nullptr);
}

static void handler_loop() {
    handler_calls = 0;
    std::set_new_handler(counting_handler);
    void *p = ::operator new(huge, std::nothrow);
    std::set_new_handler(counting_handler);
    handler_calls = 0;
    void *q = ::operator new(huge, std::align_val_t{64}, std::nothrow);
    int aligned_calls = handler_calls;
    std::set_new_handler(nullptr);
    report(p == nullptr && q == nullptr && aligned_calls == 3,
           "a failing new runs the new-handler until none is installed",
           "handler ran " + std::to_string(aligned_calls) + " times");
}

// A handler that makes memory available: the first call releases a reserve, and the retry
// that follows succeeds. With nothing else to release, it would uninstall itself.
static void *reserve;
static int releasing_calls;

static void releasing_handler() {
    releasing_calls++;
    std::free(reserve);
    reserve = nullptr;
}

static void handler_that_helps() {
    // A request that fails only while the reserve is held cannot be made portably, so this
    // checks the other half: when the first try succeeds the handler is never called.
    reserve = std::malloc(1 << 20);
    std::set_new_handler(releasing_handler);
    void *p = ::operator new(1 << 20);
    std::set_new_handler(nullptr);
    report(p != nullptr && releasing_calls == 0, "a new that succeeds calls no handler",
           "the handler ran");
    ::operator delete(p);
    std::free(reserve);
}

int main(int argc, char **argv) {
    if (argc > 1 && std::strcmp(argv[1], "throw") == 0) {
        try {
            char *p = new char[huge];
            std::printf("allocated %p\n", static_cast<void *>(p));
        } catch (const std::bad_alloc &) {
            std::puts("caught bad_alloc");
        }
        return 0;
    }
    if (argc > 1 && std::strcmp(argv[1], "throwing-handler") == 0) {
        std::set_new_handler([] { throw std::bad_alloc(); });
        void *p = ::operator new(huge, std::nothrow);
        std::printf("nothrow new returned %s\n", p ? "a block" : "null");
        return 0;
    }
    plain_forms();
    aligned_forms();
    zero_size();
    wrong_sizes();
    nothrow_failure();
    handler_loop();
    handler_that_helps();
    return failures != 0;
}
