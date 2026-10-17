//! What every workload binary shares: pinning, hardware counters over a measured region,
//! deterministic random sizes, and the one-line JSON result the runner reads.
//!
//! The protocol with the runner (bench/runner) is plain stdout lines:
//!
//! - `BENCH_RESULT {json}`: the result, printed once at the end (see [`Report`]).
//! - `BENCH_LISTEN addr`: a server's bound address, so a driver can be started against it.
//! - `BENCH_MARK name`: a driver says that a phase of the measured process begins or ends,
//!   for measurements the runner takes from outside (`warm` then `done`).
//!
//! The CPUs a workload may use come from `BENCH_CPUS` (a list such as `0-3,8-11`); thread `i`
//! of a workload is pinned to the `i`-th CPU of that list, wrapping around.

pub mod cpus;
pub mod mem;
pub mod perf;
pub mod report;
pub mod rng;

pub use cpus::Cpus;
pub use perf::{Counters, Sample};
pub use report::{Better, Report};
pub use rng::{Rng, SizeMix};

/// `key=value` command-line arguments after the workload name.
pub struct Args(Vec<(String, String)>);

impl Args {
    pub fn from_env() -> (String, Args) {
        let mut it = std::env::args().skip(1);
        let name = it.next().unwrap_or_default();
        let pairs = it
            .map(|a| match a.split_once('=') {
                Some((k, v)) => (k.to_string(), v.to_string()),
                None => panic!("argument {a:?} is not key=value"),
            })
            .collect();
        (name, Args(pairs))
    }

    pub fn str(&self, key: &str, default: &str) -> String {
        self.0.iter().rev().find(|(k, _)| k == key).map(|(_, v)| v.clone()).unwrap_or_else(|| default.to_string())
    }

    pub fn num(&self, key: &str, default: u64) -> u64 {
        match self.0.iter().rev().find(|(k, _)| k == key) {
            Some((_, v)) => parse_num(v).unwrap_or_else(|| panic!("{key}={v} is not a number")),
            None => default,
        }
    }

    pub fn usize(&self, key: &str, default: usize) -> usize {
        self.num(key, default as u64) as usize
    }

    /// Every argument, for the report.
    pub fn pairs(&self) -> &[(String, String)] {
        &self.0
    }
}

/// Parses `123`, `10k`, `4M`, `2G` (powers of 1000) and `64Ki`, `1Mi`, `1Gi` (powers of 1024),
/// and `1e6`.
pub fn parse_num(s: &str) -> Option<u64> {
    let (digits, mult) = if let Some(d) = s.strip_suffix("Ki") {
        (d, 1u64 << 10)
    } else if let Some(d) = s.strip_suffix("Mi") {
        (d, 1 << 20)
    } else if let Some(d) = s.strip_suffix("Gi") {
        (d, 1 << 30)
    } else if let Some(d) = s.strip_suffix('k') {
        (d, 1_000)
    } else if let Some(d) = s.strip_suffix('M') {
        (d, 1_000_000)
    } else if let Some(d) = s.strip_suffix('G') {
        (d, 1_000_000_000)
    } else {
        (s, 1)
    };
    if let Some((m, e)) = digits.split_once('e') {
        let m: u64 = m.parse().ok()?;
        let e: u32 = e.parse().ok()?;
        return m.checked_mul(10u64.checked_pow(e)?)?.checked_mul(mult);
    }
    digits.parse::<u64>().ok()?.checked_mul(mult)
}

#[cfg(test)]
mod tests {
    use super::parse_num;

    #[test]
    fn numbers() {
        assert_eq!(parse_num("123"), Some(123));
        assert_eq!(parse_num("10k"), Some(10_000));
        assert_eq!(parse_num("64Ki"), Some(65_536));
        assert_eq!(parse_num("1e7"), Some(10_000_000));
        assert_eq!(parse_num("x"), None);
    }
}
