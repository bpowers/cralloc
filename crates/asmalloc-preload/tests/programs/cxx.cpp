// A C++ program that exercises what C++ allocates through: containers, strings, shared
// pointers, exceptions (whose objects libstdc++ allocates), over-aligned `new` (which
// libstdc++ serves with aligned_alloc and frees with free), iostreams and threads. Prints a
// result that does not depend on scheduling.
#include <algorithm>
#include <atomic>
#include <cstdint>
#include <cstdio>
#include <map>
#include <memory>
#include <mutex>
#include <numeric>
#include <sstream>
#include <stdexcept>
#include <string>
#include <thread>
#include <unordered_map>
#include <vector>

struct alignas(64) Line {
    std::uint64_t words[8];
};

struct alignas(4096) Page {
    unsigned char bytes[4096];
};

static std::uint64_t work(unsigned seed) {
    std::uint64_t s = seed * 0x9E3779B97F4A7C15ull + 1;
    auto next = [&] {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        return s;
    };
    std::map<std::string, std::vector<int>> tree;
    std::unordered_map<std::uint64_t, std::shared_ptr<std::string>> hash;
    std::uint64_t sum = 0;
    for (int i = 0; i < 20000; i++) {
        std::string key = "key-" + std::to_string(next() % 5000);
        auto &v = tree[key];
        v.push_back(i);
        if (v.size() > 50) v.erase(v.begin(), v.begin() + 25);
        hash[next() % 3000] = std::make_shared<std::string>(key + std::string(next() % 300, 'x'));
        if (i % 97 == 0) {
            try {
                throw std::runtime_error("failure number " + std::to_string(i));
            } catch (const std::exception &e) {
                sum += std::string(e.what()).size();
            }
        }
        if (i % 13 == 0) {
            auto lines = std::make_unique<Line[]>(next() % 64 + 1);
            if (reinterpret_cast<std::uintptr_t>(lines.get()) % 64 != 0) std::abort();
            lines[0].words[0] = i;
            sum += lines[0].words[0];
        }
        if (i % 501 == 0) {
            auto page = std::make_unique<Page>();
            if (reinterpret_cast<std::uintptr_t>(page.get()) % 4096 != 0) std::abort();
            page->bytes[4095] = 1;
            sum += page->bytes[4095];
        }
    }
    for (auto &[k, v] : tree) sum += k.size() + std::accumulate(v.begin(), v.end(), 0ull);
    for (auto &[k, v] : hash) sum += k ^ v->size();
    std::ostringstream out;
    out << sum;
    return out.str().size() + sum;
}

int main() {
    std::vector<std::uint64_t> results(16);
    std::vector<std::thread> threads;
    for (unsigned t = 0; t < 16; t++) threads.emplace_back([&, t] { results[t] = work(t); });
    for (auto &t : threads) t.join();
    std::uint64_t total = 0;
    for (auto r : results) total ^= r;
    std::printf("c++ total %llu\n", static_cast<unsigned long long>(total));
}
