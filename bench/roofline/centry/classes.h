/* Size classes as crates/asmalloc/src/classes defines them, and the tables the generated
 * routines read (see gen.py for their layout). Shared by the driver and the stand-in. */
#ifndef CENTRY_CLASSES_H
#define CENTRY_CLASSES_H

#include <stddef.h>
#include <stdint.h>
#include <sys/rseq.h>

#include "front.h"

enum { CLASS_COUNT = 112, MAX_SMALL_SIZE = 32 << 10 };

static inline unsigned class_of_size(size_t size) {
    size_t words = (size + 7) / 8;
    if (words <= 8) return words == 0 ? 1 : (unsigned)words;
    size_t w = words - 1;
    unsigned b = 63 - (unsigned)__builtin_clzl(w);
    return 4 * b + (unsigned)((w >> (b - 2)) & 3) - 3;
}

/* C's rule: a block of 16 bytes or more is 16-byte aligned. */
static inline unsigned class_of_c(size_t size) {
    if (size == 0) size = 1;
    if (size >= 16) size = (size + 15) & ~(size_t)15;
    return class_of_size(size);
}

static inline size_t class_size(unsigned c) {
    if (c <= 8) return 8 * (size_t)c;
    unsigned k = c + 3, b = k >> 2, m = k & 3;
    return ((size_t)(5 + m) << (b - 2)) * 8;
}

static inline unsigned mag_slots(unsigned c) {
    size_t fit = (128 << 10) / class_size(c) + 2;
    unsigned log2 = 63 - (unsigned)__builtin_clzl(fit);
    if (log2 < 2) log2 = 2;
    if (log2 > 8) log2 = 8;
    return 1u << log2;
}

static inline uint32_t mag_mask(unsigned c) { return 8 * mag_slots(c) - 16; }

/* Fills one copy's tables for a heap at `base` with regions of 2^shift bytes, region c holding
 * class c, and publishes the heap word last: until then every free takes the slow path. */
static inline void setup_tables(struct front_data *d, uintptr_t base, unsigned shift) {
    char *slabs = d->slabs;
    uint8_t *region = (uint8_t *)slabs + OFF_REGION;
    uint64_t *masks8 = (uint64_t *)(slabs + OFF_MASK);
    uint8_t *size_s = (uint8_t *)slabs + OFF_SIZE_S, *size_t_ = (uint8_t *)slabs + OFF_SIZE_T;
    uint8_t *size_d = (uint8_t *)slabs + OFF_SIZE_D;
    *(uint64_t *)d->shift = shift;
    *(uint64_t *)d->baseidx = base >> shift;
    *(int64_t *)d->rseq_off = __rseq_offset;
    *(uintptr_t *)d->regbias = (uintptr_t)region - (base >> shift);
    for (unsigned r = 0; r < 4096; r++) {
        uint8_t tag = r <= CLASS_COUNT ? (uint8_t)r : 0;
        region[r] = tag;
        d->regclass_sep[r] = (char)tag;
    }
    for (unsigned c = 0; c < 256; c++) {
        uint32_t m = (c >= 1 && c <= SMALL) ? mag_mask(c) : 0;
        masks8[c] = m;
        ((uint32_t *)d->masks4)[c] = m;
    }
    for (unsigned i = 0; i <= 128; i++) {
        unsigned c = class_of_c(8 * (size_t)i);
        size_s[i] = (uint8_t)c;
        d->size_s_sep[i] = (char)c;
        size_t_[i] = (uint8_t)c;
        ((uint32_t *)d->packed)[2 * i] = 8 * c;
        ((uint32_t *)d->packed)[2 * i + 1] = mag_mask(c);
        ((char **)d->sizeptr)[i] = slabs + 8 * c;
    }
    for (unsigned j = 1; j <= 248; j++) size_t_[128 + j] = (uint8_t)class_of_c(1024 + 128 * (size_t)j);
    for (unsigned n = 0; n <= 1024; n++) size_d[n] = (uint8_t)class_of_c(n);
    uint8_t *size_l = (uint8_t *)slabs + OFF_SIZE_L;
    for (unsigned i = 0; i <= MAX_SMALL_SIZE / 8; i++) size_l[i] = (uint8_t)class_of_c(8 * (size_t)i);
    __atomic_store_n((uint64_t *)d->space, (uint64_t)base | shift, __ATOMIC_RELEASE);
}

#endif
