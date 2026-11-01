// An ordinary C++ program, to be linked against another allocator's shared library.
#include <cstdio>
#include <string>
#include <vector>
int main() {
    std::vector<std::string> v;
    for (int i = 0; i < 1000; i++) v.push_back(std::string(i % 200, 'x'));
    std::size_t n = 0;
    for (auto &s : v) n += s.size();
    std::printf("%zu\n", n);
    return 0;
}
