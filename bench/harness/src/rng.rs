//! Deterministic pseudo-random numbers and object-size distributions, so that every
//! allocator sees exactly the same request sequence.

/// SplitMix64: tiny, fast, and good enough to choose sizes and victims.
#[derive(Clone, Debug)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed ^ 0x9E37_79B9_7F4A_7C15)
    }

    #[inline]
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in `0..n`.
    #[inline]
    pub fn below(&mut self, n: u64) -> u64 {
        ((self.next_u64() as u128 * n as u128) >> 64) as u64
    }

    /// Uniform in `[0, 1)`.
    #[inline]
    pub fn unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
}

/// A distribution of object sizes in bytes. Every size is at least 16, so an object can
/// carry its own size and a sequence number.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SizeMix {
    /// Always this many bytes.
    Fixed(usize),
    /// Uniform over the small classes Rust programs use most: 16 to 256 bytes.
    Small,
    /// Mostly small, with a tail of medium objects: 80% 16 to 128 bytes, 15% 128 to 1024,
    /// 5% 1 to 16 KiB, log-uniform within each band, rounded to 8 bytes.
    Mixed,
    /// Buffers: log-uniform from 4 KiB to 256 KiB.
    Large,
}

impl SizeMix {
    /// `s64` (or any `sN`), `small`, `mixed`, `large`.
    pub fn parse(s: &str) -> Option<SizeMix> {
        match s {
            "small" => Some(SizeMix::Small),
            "mixed" => Some(SizeMix::Mixed),
            "large" => Some(SizeMix::Large),
            _ => s.strip_prefix('s').and_then(|n| n.parse().ok()).filter(|&n| n >= 16).map(SizeMix::Fixed),
        }
    }

    pub fn name(&self) -> String {
        match self {
            SizeMix::Fixed(n) => format!("s{n}"),
            SizeMix::Small => "small".into(),
            SizeMix::Mixed => "mixed".into(),
            SizeMix::Large => "large".into(),
        }
    }

    #[inline]
    pub fn sample(&self, rng: &mut Rng) -> usize {
        const SMALL: [usize; 12] = [16, 24, 32, 40, 48, 64, 80, 96, 128, 160, 192, 256];
        let log_uniform = |rng: &mut Rng, lo: f64, hi: f64| -> usize {
            let x = (lo.ln() + rng.unit() * (hi.ln() - lo.ln())).exp() as usize;
            (x + 7) & !7
        };
        match *self {
            SizeMix::Fixed(n) => n,
            SizeMix::Small => SMALL[rng.below(SMALL.len() as u64) as usize],
            SizeMix::Mixed => {
                let r = rng.below(100);
                if r < 80 {
                    log_uniform(rng, 16.0, 128.0)
                } else if r < 95 {
                    log_uniform(rng, 128.0, 1024.0)
                } else {
                    log_uniform(rng, 1024.0, 16384.0)
                }
            }
            SizeMix::Large => log_uniform(rng, 4096.0, 262_144.0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_are_in_range_and_repeatable() {
        for mix in [SizeMix::Fixed(64), SizeMix::Small, SizeMix::Mixed, SizeMix::Large] {
            let (mut a, mut b) = (Rng::new(1), Rng::new(1));
            for _ in 0..10_000 {
                let s = mix.sample(&mut a);
                assert_eq!(s, mix.sample(&mut b));
                assert!((16..=262_144).contains(&s) && s % 8 == 0, "{mix:?} gave {s}");
            }
        }
        assert_eq!(SizeMix::parse("s64"), Some(SizeMix::Fixed(64)));
        assert_eq!(SizeMix::parse("s8"), None);
    }
}
