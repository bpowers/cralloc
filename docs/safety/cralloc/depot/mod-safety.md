# Safety ledger: `crates/cralloc/src/depot/mod.rs`

## CACHE-01: `Cache::with_tuning` in `Cache::new`

**Proof.** `Cache::new` has the same contract as `Cache::with_tuning` and passes its caller's
promise on unchanged. The contract itself is what MAGAZINE-01 (the committed mark describes this
space) and the slab invariant (S) in `front/real-safety.md` (slab words and spare links
name this area's magazines) rely on.

Reviewed: a09c36e