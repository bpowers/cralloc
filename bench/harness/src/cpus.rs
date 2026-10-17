//! CPU lists and pinning.

/// An ordered list of CPUs. Order matters: thread `i` goes on `list[i % len]`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cpus(pub Vec<usize>);

impl Cpus {
    /// Parses `0-3,8,10-11`. Order is kept, so `0,8,1,9` alternates two L3 domains.
    pub fn parse(s: &str) -> Option<Cpus> {
        let mut v = Vec::new();
        for part in s.split(',').map(str::trim).filter(|p| !p.is_empty()) {
            match part.split_once('-') {
                Some((a, b)) => {
                    let (a, b): (usize, usize) = (a.parse().ok()?, b.parse().ok()?);
                    if a > b {
                        return None;
                    }
                    v.extend(a..=b);
                }
                None => v.push(part.parse().ok()?),
            }
        }
        (!v.is_empty()).then_some(Cpus(v))
    }

    /// `BENCH_CPUS`, or the CPUs this process may run on.
    pub fn from_env() -> Cpus {
        match std::env::var("BENCH_CPUS") {
            Ok(s) => Cpus::parse(&s).unwrap_or_else(|| panic!("BENCH_CPUS={s:?} is not a CPU list")),
            Err(_) => Cpus::allowed(),
        }
    }

    /// The current thread's affinity mask.
    pub fn allowed() -> Cpus {
        // SAFETY: cpu_set_t is plain data, and sched_getaffinity writes at most its size.
        unsafe {
            let mut set: libc::cpu_set_t = std::mem::zeroed();
            libc::sched_getaffinity(0, std::mem::size_of::<libc::cpu_set_t>(), &mut set);
            Cpus((0..libc::CPU_SETSIZE as usize).filter(|&c| libc::CPU_ISSET(c, &set)).collect())
        }
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The CPU for thread `i`.
    pub fn nth(&self, i: usize) -> usize {
        self.0[i % self.0.len()]
    }

    /// Pins the calling thread to the CPU for thread `i`.
    pub fn pin(&self, i: usize) {
        pin_to(&[self.nth(i)]);
    }

    pub fn to_list(&self) -> String {
        self.0.iter().map(|c| c.to_string()).collect::<Vec<_>>().join(",")
    }
}

/// Restricts the calling thread to `cpus`.
pub fn pin_to(cpus: &[usize]) {
    // SAFETY: as in `allowed`; CPU_SET bounds-checks against CPU_SETSIZE.
    unsafe {
        let mut set: libc::cpu_set_t = std::mem::zeroed();
        for &c in cpus {
            libc::CPU_SET(c, &mut set);
        }
        let r = libc::sched_setaffinity(0, std::mem::size_of::<libc::cpu_set_t>(), &set);
        assert_eq!(r, 0, "sched_setaffinity({cpus:?}) failed");
    }
}

#[cfg(test)]
mod tests {
    use super::Cpus;

    #[test]
    fn parse_keeps_order() {
        assert_eq!(Cpus::parse("0-2,8").unwrap().0, vec![0, 1, 2, 8]);
        assert_eq!(Cpus::parse("0,8,1,9").unwrap().nth(5), 8);
        assert!(Cpus::parse("3-1").is_none());
    }
}
