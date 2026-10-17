//! Medians, quantiles and bootstrap intervals.
use bench_harness::rng::Rng;

/// The `q` quantile (0 to 1) by linear interpolation. `v` need not be sorted.
pub fn quantile(v: &[f64], q: f64) -> f64 {
    if v.is_empty() {
        return f64::NAN;
    }
    let mut s = v.to_vec();
    s.sort_by(|a, b| a.total_cmp(b));
    let pos = q * (s.len() - 1) as f64;
    let (lo, hi) = (pos.floor() as usize, pos.ceil() as usize);
    s[lo] + (s[hi] - s[lo]) * (pos - lo as f64)
}

pub fn median(v: &[f64]) -> f64 {
    quantile(v, 0.5)
}

/// Half the interquartile range as a fraction of the median: the spread a reader can
/// compare against differences between allocators.
pub fn half_iqr_rel(v: &[f64]) -> f64 {
    (quantile(v, 0.75) - quantile(v, 0.25)) / 2.0 / median(v).abs()
}

/// A 95% percentile-bootstrap interval for the median of `v` (NaN with fewer than three).
pub fn median_ci(v: &[f64], seed: u64) -> (f64, f64) {
    if v.len() < 3 {
        return (f64::NAN, f64::NAN);
    }
    let mut rng = Rng::new(seed);
    let meds: Vec<f64> = (0..2000)
        .map(|_| median(&(0..v.len()).map(|_| v[rng.below(v.len() as u64) as usize]).collect::<Vec<f64>>()))
        .collect();
    (quantile(&meds, 0.025), quantile(&meds, 0.975))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quantiles() {
        assert_eq!(median(&[3.0, 1.0, 2.0]), 2.0);
        assert_eq!(median(&[1.0, 2.0, 3.0, 4.0]), 2.5);
        assert_eq!(quantile(&[1.0, 2.0, 3.0, 4.0, 5.0], 0.25), 2.0);
        assert_eq!(median_ci(&[1.0; 8], 1), (1.0, 1.0));
        assert!(median_ci(&[1.0, 2.0], 1).0.is_nan());
    }
}
