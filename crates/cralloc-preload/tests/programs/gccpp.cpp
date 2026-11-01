// A program that relies on libgccpp's global operator new: with it, `new` returns memory the
// collector scans (uncollectable), so collected objects reachable only from such memory stay
// alive. Builds lists of collectable nodes hanging off `new`ed holders, collects, and checks.
#include <gc_cpp.h>
#include <cstdio>
#include <cstring>

struct Node {
    Node *next;
    long value;
};

struct Holder {
    Node *head;  // points into the collected heap
    long count;
};

int main() {
    GC_INIT();
    const int holders = 2000, per = 50;
    Holder **all = new Holder *[holders];
    for (int h = 0; h < holders; h++) {
        all[h] = new Holder{nullptr, 0};
        for (int i = 0; i < per; i++) {
            Node *n = static_cast<Node *>(GC_MALLOC(sizeof(Node)));
            n->next = all[h]->head;
            n->value = (long)h * per + i;
            all[h]->head = n;
            all[h]->count++;
        }
    }
    for (int round = 0; round < 5; round++) {
        GC_gcollect();
        // Garbage of the same size, so a node the collector freed is reused and overwritten.
        for (int i = 0; i < 200000; i++) {
            Node *g = static_cast<Node *>(GC_MALLOC(sizeof(Node)));
            g->next = nullptr;
            g->value = -1;
        }
    }
    long bad = 0;
    for (int h = 0; h < holders; h++) {
        long i = per - 1;
        for (Node *n = all[h]->head; n; n = n->next, i--)
            if (n->value != (long)h * per + i) bad++;
        delete all[h];
    }
    delete[] all;
    std::printf("nodes damaged: %ld\n", bad);
    return bad != 0;
}
