//! Deterministic entropy for operation streams.
//!
//! Every decision a stream makes (operation, size, alignment, victim block, entry point) is
//! drawn from an [`Entropy`] source. A seeded [`Rng`] makes a run reproducible from its seed; a
//! [`ByteSource`] over a fuzzer's input makes each mutation of the input a mutation of the
//! operation stream.

/// A stream of 32-bit values that may run dry.
pub trait Entropy {
    /// Next value, or `None` once the source is exhausted (a PRNG never is).
    fn next_u32(&mut self) -> Option<u32>;

    /// Uniform in `0..n` for non-zero `n`. Lemire's multiply-shift: its bias is far below
    /// anything a test could observe, and it needs no rejection loop that could consume an
    /// unbounded amount of a fuzzer's input.
    fn below(&mut self, n: u32) -> Option<u32> {
        debug_assert!(n > 0);
        self.next_u32()
            .map(|x| ((u64::from(x) * u64::from(n)) >> 32) as u32)
    }

    /// Uniform in `lo..=hi`. The range must be at most `2^32` wide.
    fn range(&mut self, lo: usize, hi: usize) -> Option<usize> {
        assert!(lo <= hi, "empty range {lo}..={hi}");
        let width = hi - lo;
        assert!(width <= u32::MAX as usize, "range {lo}..={hi} is too wide");
        if width == u32::MAX as usize {
            return self.next_u32().map(|x| lo + x as usize);
        }
        self.below(width as u32 + 1).map(|x| lo + x as usize)
    }

    /// An index chosen with probability proportional to `weights[i]`. Entries with weight zero
    /// are never chosen; the weights must not all be zero.
    fn weighted(&mut self, weights: &[u32]) -> Option<usize> {
        let total: u32 = weights.iter().sum();
        assert!(total > 0, "all weights are zero");
        let mut x = self.below(total)?;
        for (i, &w) in weights.iter().enumerate() {
            if x < w {
                return Some(i);
            }
            x -= w;
        }
        unreachable!("x < total = sum of weights")
    }
}

/// splitmix64's output mixer (Steele, Lea and Flood, 2014): a bijection on `u64` with good
/// avalanche, so distinct inputs give distinct, uncorrelated outputs. Block patterns use it too.
pub const fn mix64(mut z: u64) -> u64 {
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// splitmix64: a Weyl sequence through [`mix64`]. Full period and no bad seeds.
#[derive(Clone, Debug)]
pub struct Rng {
    state: u64,
}

impl Rng {
    /// Start from `seed`; every seed is valid.
    pub const fn new(seed: u64) -> Self {
        Rng { state: seed }
    }

    /// Next 64 random bits.
    pub fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        mix64(self.state)
    }
}

impl Entropy for Rng {
    fn next_u32(&mut self) -> Option<u32> {
        Some((self.next_u64() >> 32) as u32)
    }
}

/// Little-endian `u32`s read off a byte slice; runs dry when fewer than four bytes remain.
#[derive(Clone, Debug)]
pub struct ByteSource<'a> {
    data: &'a [u8],
}

impl<'a> ByteSource<'a> {
    /// Read from `data`.
    pub const fn new(data: &'a [u8]) -> Self {
        ByteSource { data }
    }

    /// Bytes not yet consumed.
    pub fn remaining(&self) -> usize {
        self.data.len()
    }
}

impl Entropy for ByteSource<'_> {
    fn next_u32(&mut self) -> Option<u32> {
        let (head, tail) = self.data.split_first_chunk::<4>()?;
        self.data = tail;
        Some(u32::from_le_bytes(*head))
    }
}

/// Either kind of source, so a stream's owner need not be generic over it.
#[derive(Clone, Debug)]
pub enum AnyEntropy<'a> {
    /// A seeded PRNG.
    Rng(Rng),
    /// A fuzzer's bytes.
    Bytes(ByteSource<'a>),
}

impl Entropy for AnyEntropy<'_> {
    fn next_u32(&mut self) -> Option<u32> {
        match self {
            AnyEntropy::Rng(r) => r.next_u32(),
            AnyEntropy::Bytes(b) => b.next_u32(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rng_is_deterministic_and_seed_sensitive() {
        let draw = |seed| {
            let mut r = Rng::new(seed);
            (0..8).map(|_| r.next_u64()).collect::<Vec<_>>()
        };
        assert_eq!(draw(42), draw(42));
        assert_ne!(draw(42), draw(43));
        let zero = draw(0);
        assert_ne!(zero[0], zero[1], "the zero seed is not degenerate");
    }

    #[test]
    fn below_and_range_stay_in_bounds() {
        let mut r = Rng::new(7);
        for _ in 0..10_000 {
            assert!(r.below(3).unwrap() < 3);
            assert!((10..=12).contains(&r.range(10, 12).unwrap()));
            assert_eq!(r.range(5, 5).unwrap(), 5);
        }
        let full = r.range(0, u32::MAX as usize).unwrap();
        assert!(full <= u32::MAX as usize);
    }

    #[test]
    fn weighted_never_picks_zero_weight_and_hits_every_positive_weight() {
        let mut r = Rng::new(1);
        let mut hits = [0usize; 4];
        for _ in 0..20_000 {
            hits[r.weighted(&[1, 0, 3, 4]).unwrap()] += 1;
        }
        assert_eq!(hits[1], 0);
        assert!(hits[0] > 0 && hits[2] > hits[0] && hits[3] > hits[2]);
    }

    #[test]
    fn byte_source_runs_dry_on_a_short_tail() {
        let mut s = ByteSource::new(&[1, 0, 0, 0, 2, 0, 0, 0, 9, 9]);
        assert_eq!(s.next_u32(), Some(1));
        assert_eq!(s.next_u32(), Some(2));
        assert_eq!(s.remaining(), 2);
        assert_eq!(s.next_u32(), None);
        assert_eq!(s.below(10), None);
    }
}
