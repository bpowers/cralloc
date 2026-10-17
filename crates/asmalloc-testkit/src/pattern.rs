//! Block contents: a pattern whose every byte can be recomputed from the block's seed.
//!
//! The model fills each block it holds with a pattern and checks it every time it touches the
//! block, so a byte the allocator writes into a live block is caught at the next touch. The seed
//! comes from the block's address, its length and a serial number ([`seed`]), so a stale copy of
//! an earlier block at the same place, a copy of another block, and a shifted copy all fail.
//!
//! So that debug builds stay fast on multi-megabyte blocks, the pattern is dense over the first
//! [`DENSE_PREFIX`] bytes and sampled beyond that: one 8-byte word every [`STRIDE`] bytes, plus
//! the word holding the last byte. Sampled positions depend only on the offset, never on the
//! block length (except the last word), so a prefix of a block can be checked after a resize has
//! changed its length. The price is that a change to fewer than [`STRIDE`] bytes past the dense
//! prefix can go unnoticed; alignment, overlap and the dense prefix are always checked exactly.

use crate::entropy::mix64;

/// Bytes at the start of every block whose pattern is written and checked densely.
pub const DENSE_PREFIX: usize = 64 * 1024;
/// Beyond the dense prefix, one 8-byte word is written and checked every this many bytes.
pub const STRIDE: usize = 4096;

const GOLDEN: u64 = 0x9E37_79B9_7F4A_7C15;

/// The pattern seed of a block at `addr` of `len` bytes, filled for the `serial`-th time.
pub fn seed(addr: usize, len: usize, serial: u64) -> u64 {
    mix64(mix64(addr as u64) ^ mix64(len as u64 ^ GOLDEN) ^ serial)
}

/// What a range of a block should hold.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Contents {
    /// The pattern with this seed.
    Pattern(u64),
    /// Zero bytes.
    Zero,
}

impl Contents {
    /// The word covering bytes `8 * chunk .. 8 * chunk + 8`. XOR with a multiple of an odd
    /// constant makes every word of a block distinct, so a shifted copy never matches.
    fn word(self, chunk: usize) -> u64 {
        match self {
            Contents::Pattern(seed) => seed ^ (chunk as u64).wrapping_mul(GOLDEN),
            Contents::Zero => 0,
        }
    }

    fn byte(self, offset: usize) -> u8 {
        self.word(offset / 8).to_le_bytes()[offset % 8]
    }
}

/// The first byte that did not hold what it should.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mismatch {
    /// Offset from the start of the block.
    pub offset: usize,
    /// The byte the model expected.
    pub expected: u8,
    /// The byte the block held.
    pub found: u8,
}

/// Calls `f` on each byte range of `lo..hi` that the pattern covers in a block of `len` bytes,
/// in increasing order and without overlap.
fn for_each_span<E>(
    len: usize,
    lo: usize,
    hi: usize,
    mut f: impl FnMut(usize, usize) -> Result<(), E>,
) -> Result<(), E> {
    debug_assert!(lo <= hi && hi <= len, "{lo}..{hi} of {len}");
    let dense_end = hi.min(DENSE_PREFIX);
    let mut covered = lo;
    if lo < dense_end {
        f(lo, dense_end)?;
        covered = dense_end;
    }
    if hi > DENSE_PREFIX {
        let first = lo.saturating_sub(DENSE_PREFIX) / STRIDE;
        let mut word = DENSE_PREFIX + first * STRIDE;
        while word < hi {
            let (a, b) = (word.max(covered), (word + 8).min(hi));
            if a < b {
                f(a, b)?;
                covered = b;
            }
            word += STRIDE;
        }
    }
    // The word holding the last byte is the one position that depends on the length, so it is
    // covered only when the range reaches the end of the block.
    if hi == len && len > DENSE_PREFIX {
        let a = ((len - 1) & !7).max(covered);
        if a < len {
            f(a, len)?;
        }
    }
    Ok(())
}

/// Write the pattern of `seed` over the `len`-byte block at `p`.
///
/// # Safety
///
/// `p` must be valid for writes of `len` bytes.
pub unsafe fn fill(p: *mut u8, len: usize, seed: u64) {
    let contents = Contents::Pattern(seed);
    let _ = for_each_span::<()>(len, 0, len, |a, b| {
        // SAFETY: `a..b` lies inside the block, which the caller vouches for.
        unsafe { write_range(p, a, b, contents) };
        Ok(())
    });
}

/// Check bytes `lo..hi` of the `len`-byte block at `p` against `contents`, at the positions
/// the pattern covers.
///
/// # Safety
///
/// `p` must be valid for reads of `hi` bytes, and those bytes must be initialized.
pub unsafe fn check(
    p: *const u8,
    len: usize,
    lo: usize,
    hi: usize,
    contents: Contents,
) -> Result<(), Mismatch> {
    for_each_span(len, lo, hi, |a, b| {
        // SAFETY: `a..b` lies inside `lo..hi`, which the caller vouches for.
        unsafe { check_range(p, a, b, contents) }
    })
}

/// # Safety
///
/// `p` must be valid for writes of bytes `a..b`.
unsafe fn write_range(p: *mut u8, a: usize, b: usize, contents: Contents) {
    let mut off = a;
    // SAFETY (all three loops): every offset written is in `a..b`.
    while off < b && !off.is_multiple_of(8) {
        unsafe { p.add(off).write(contents.byte(off)) };
        off += 1;
    }
    while off + 8 <= b {
        let w = contents.word(off / 8).to_le();
        unsafe { p.add(off).cast::<u64>().write_unaligned(w) };
        off += 8;
    }
    while off < b {
        unsafe { p.add(off).write(contents.byte(off)) };
        off += 1;
    }
}

/// # Safety
///
/// `p` must be valid for reads of the initialized bytes `a..b`.
unsafe fn check_range(
    p: *const u8,
    a: usize,
    b: usize,
    contents: Contents,
) -> Result<(), Mismatch> {
    let byte_at = |off: usize| {
        // SAFETY: callers below pass only offsets in `a..b`.
        let found = unsafe { p.add(off).read() };
        let expected = contents.byte(off);
        if found == expected {
            Ok(())
        } else {
            Err(Mismatch {
                offset: off,
                expected,
                found,
            })
        }
    };
    let mut off = a;
    while off < b && !off.is_multiple_of(8) {
        byte_at(off)?;
        off += 1;
    }
    while off + 8 <= b {
        // SAFETY: `off + 8 <= b`.
        let found = u64::from_le(unsafe { p.add(off).cast::<u64>().read_unaligned() });
        if found != contents.word(off / 8) {
            for i in off..off + 8 {
                byte_at(i)?;
            }
        }
        off += 8;
    }
    while off < b {
        byte_at(off)?;
        off += 1;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn covered(len: usize, lo: usize, hi: usize) -> Vec<bool> {
        let mut c = vec![false; len];
        let mut last_end = 0;
        for_each_span::<()>(len, lo, hi, |a, b| {
            assert!(
                lo <= a && a < b && b <= hi,
                "span {a}..{b} for {lo}..{hi} of {len}"
            );
            assert!(a >= last_end, "spans overlap or go backwards");
            last_end = b;
            c[a..b].iter_mut().for_each(|x| *x = true);
            Ok(())
        })
        .unwrap();
        c
    }

    #[test]
    fn small_blocks_are_dense_and_large_ones_sampled() {
        for len in [1, 7, 8, 9, 100, DENSE_PREFIX - 1, DENSE_PREFIX] {
            assert!(covered(len, 0, len).iter().all(|&x| x), "len {len}");
        }
        let len = DENSE_PREFIX + 3 * STRIDE + 29;
        let c = covered(len, 0, len);
        assert!(c[..DENSE_PREFIX].iter().all(|&x| x));
        let tail = (len - 1) & !7;
        for s in (DENSE_PREFIX..len).step_by(STRIDE) {
            assert!(
                c[s..(s + 8).min(len)].iter().all(|&x| x),
                "stride word at {s}"
            );
            if s + 8 < tail {
                assert!(!c[s + 8], "byte after the stride word at {s}");
            }
        }
        assert!(c[tail..].iter().all(|&x| x));
        assert!(!c[tail - 1]);
    }

    #[test]
    fn a_prefix_covers_partial_stride_words_but_never_the_tail() {
        let len = DENSE_PREFIX + 5 * STRIDE + 100;
        let hi = DENSE_PREFIX + STRIDE + 4;
        let c = covered(len, 0, hi);
        assert!(c[DENSE_PREFIX + STRIDE..hi].iter().all(|&x| x));
        assert!(!c[hi..].iter().any(|&x| x));
        let mid = covered(len, DENSE_PREFIX + 3, len);
        assert!(mid[DENSE_PREFIX + 3..DENSE_PREFIX + 8].iter().all(|&x| x));
        assert!(!mid[..DENSE_PREFIX + 3].iter().any(|&x| x));
    }

    #[test]
    fn fill_and_check_agree_and_catch_single_byte_changes() {
        let len = DENSE_PREFIX + 2 * STRIDE + 5;
        let mut buf = vec![0u8; len];
        let seed = 0xDEAD_BEEF_CAFE_F00D;
        // SAFETY: `buf` has `len` bytes.
        unsafe { fill(buf.as_mut_ptr(), len, seed) };
        let check_all = |b: &[u8], s| {
            // SAFETY: `b` has `len` initialized bytes.
            unsafe { check(b.as_ptr(), len, 0, len, Contents::Pattern(s)) }
        };
        assert_eq!(check_all(&buf, seed), Ok(()));
        for (offset, sampled) in [
            (0, true),
            (DENSE_PREFIX - 1, true),
            (DENSE_PREFIX + 3, true),
            (DENSE_PREFIX + STRIDE - 1, false),
            (len - 1, true),
        ] {
            let mut copy = buf.clone();
            copy[offset] ^= 0x5A;
            let r = check_all(&copy, seed);
            if sampled {
                let m = r.expect_err("a change at a sampled offset must be caught");
                assert_eq!(
                    (m.offset, m.expected, m.found),
                    (offset, buf[offset], copy[offset])
                );
            } else {
                assert_eq!(r, Ok(()), "offset {offset} is deliberately unsampled");
            }
        }
        assert!(check_all(&buf, seed + 1).is_err());
        let mut shifted = vec![0u8; len];
        shifted[8..].copy_from_slice(&buf[..len - 8]);
        assert!(check_all(&shifted, seed).is_err());
    }

    #[test]
    fn seeds_differ_by_address_length_and_serial() {
        let s = seed(0x1000, 64, 1);
        assert_ne!(s, seed(0x1008, 64, 1));
        assert_ne!(s, seed(0x1000, 65, 1));
        assert_ne!(s, seed(0x1000, 64, 2));
    }

    #[test]
    fn zero_check_reads_the_same_positions_from_an_offset() {
        let len = DENSE_PREFIX + STRIDE + 8;
        let mut buf = vec![0u8; len];
        buf[3] = 1;
        // SAFETY: `buf` has `len` initialized bytes.
        let r = unsafe { check(buf.as_ptr(), len, 4, len, Contents::Zero) };
        assert_eq!(r, Ok(()), "byte 3 is below the checked range");
        buf[DENSE_PREFIX + STRIDE + 2] = 1;
        // SAFETY: as above.
        let m = unsafe { check(buf.as_ptr(), len, 4, len, Contents::Zero) }.unwrap_err();
        assert_eq!(
            (m.offset, m.expected, m.found),
            (DENSE_PREFIX + STRIDE + 2, 0, 1)
        );
    }
}
