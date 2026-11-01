// The standard containers from several threads, with objects made on one thread and destroyed
// on another: every node, string and control block goes through operator new and a (sized)
// operator delete. Prints a result that does not depend on scheduling.
#include <algorithm>
#include <condition_variable>
#include <cstdint>
#include <cstdio>
#include <deque>
#include <functional>
#include <list>
#include <map>
#include <memory>
#include <mutex>
#include <set>
#include <string>
#include <thread>
#include <unordered_map>
#include <vector>

struct Parcel {
    std::vector<std::string> words;
    std::map<int, std::string> index;
    std::shared_ptr<std::vector<std::uint64_t>> shared;
    std::function<std::uint64_t(std::uint64_t)> step;
};

// A bounded queue between neighbouring threads, so every parcel dies on another thread.
class Channel {
  public:
    void put(std::unique_ptr<Parcel> p) {
        std::unique_lock<std::mutex> lock(mutex_);
        space_.wait(lock, [&] { return items_.size() < 64; });
        items_.push_back(std::move(p));
        ready_.notify_one();
    }
    std::unique_ptr<Parcel> take() {
        std::unique_lock<std::mutex> lock(mutex_);
        ready_.wait(lock, [&] { return !items_.empty(); });
        auto p = std::move(items_.front());
        items_.pop_front();
        space_.notify_one();
        return p;
    }

  private:
    std::mutex mutex_;
    std::condition_variable ready_, space_;
    std::deque<std::unique_ptr<Parcel>> items_;
};

static std::uint64_t next(std::uint64_t &s) {
    s ^= s << 13;
    s ^= s >> 7;
    s ^= s << 17;
    return s;
}

static constexpr int kThreads = 8;
static constexpr int kParcels = 4000;

static std::uint64_t worker(int me, Channel *channels) {
    std::uint64_t s = 0x9E3779B97F4A7C15ull * static_cast<std::uint64_t>(me + 1);
    std::uint64_t digest = 0;
    std::unordered_map<std::string, std::list<int>> local;
    std::set<std::string> seen;
    for (int i = 0; i < kParcels; i++) {
        auto p = std::make_unique<Parcel>();
        int words = static_cast<int>(next(s) % 40);
        for (int w = 0; w < words; w++)
            p->words.push_back(std::string(next(s) % 200, static_cast<char>('a' + w % 26)));
        for (int k = 0; k < 20; k++) p->index[static_cast<int>(next(s) % 1000)] = std::to_string(k);
        p->shared = std::make_shared<std::vector<std::uint64_t>>(next(s) % 500, i);
        std::uint64_t salt = next(s);
        p->step = [salt, copy = p->words](std::uint64_t x) { return x * 31 + salt + copy.size(); };
        channels[(me + 1) % kThreads].put(std::move(p));

        auto q = channels[me].take();
        std::uint64_t d = 0;
        for (auto &w : q->words) d += w.size();
        for (auto &[k, v] : q->index) d += static_cast<std::uint64_t>(k) + v.size();
        d += q->shared->size();
        d = q->step(d);
        digest += d;
        auto &bucket = local[std::to_string(d % 997)];
        bucket.push_back(i);
        if (bucket.size() > 30) bucket.erase(bucket.begin(), std::next(bucket.begin(), 15));
        seen.insert(std::to_string(d % 5000));
        std::sort(q->words.begin(), q->words.end());
        q->words.erase(std::unique(q->words.begin(), q->words.end()), q->words.end());
        digest += q->words.size();
    }
    std::uint64_t sizes = 0;
    for (auto &[k, v] : local) sizes += k.size() + v.size();
    return digest ^ (sizes << 1) ^ (seen.size() << 7);
}

int main() {
    std::vector<Channel> channels(kThreads);
    std::vector<std::uint64_t> results(kThreads);
    std::vector<std::thread> threads;
    for (int t = 0; t < kThreads; t++)
        threads.emplace_back([&, t] { results[t] = worker(t, channels.data()); });
    for (auto &t : threads) t.join();
    // Each thread's digest depends only on what its neighbour made, which is deterministic.
    std::uint64_t total = 0;
    for (auto r : results) total ^= r;
    std::printf("containers %d threads %d parcels each, digest %llu\n", kThreads, kParcels,
                static_cast<unsigned long long>(total));
}
