//! Resident memory, from `/proc`.

/// Fields of `/proc/<pid>/smaps_rollup`, in bytes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rollup {
    pub rss: u64,
    pub pss: u64,
    pub anonymous: u64,
    /// Anonymous memory backed by transparent huge pages.
    pub anon_huge: u64,
    pub private_dirty: u64,
}

impl Rollup {
    /// Reads `/proc/<pid>/smaps_rollup` (`pid` 0 means this process). Costs a walk of the
    /// page tables, so do not call it in a hot loop.
    pub fn read(pid: i32) -> Option<Rollup> {
        let path = if pid == 0 { "/proc/self/smaps_rollup".to_string() } else { format!("/proc/{pid}/smaps_rollup") };
        let text = std::fs::read_to_string(path).ok()?;
        Some(Rollup::parse(&text))
    }

    pub fn parse(text: &str) -> Rollup {
        let mut r = Rollup::default();
        for line in text.lines() {
            let mut it = line.split_whitespace();
            let (Some(key), Some(val)) = (it.next(), it.next()) else { continue };
            let Ok(kb) = val.parse::<u64>() else { continue };
            let bytes = kb * 1024;
            match key {
                "Rss:" => r.rss = bytes,
                "Pss:" => r.pss = bytes,
                "Anonymous:" => r.anonymous = bytes,
                "AnonHugePages:" => r.anon_huge = bytes,
                "Private_Dirty:" => r.private_dirty = bytes,
                _ => {}
            }
        }
        r
    }
}

/// Peak resident set (`VmHWM`) in bytes of `/proc/<pid>/status` (`pid` 0: this process).
///
/// This is the peak of the current program image only. `getrusage`'s `ru_maxrss` is not a
/// substitute for a child the runner forked: it starts from the runner's own resident set
/// at the fork.
pub fn vm_hwm(pid: i32) -> Option<u64> {
    let path = if pid == 0 { "/proc/self/status".to_string() } else { format!("/proc/{pid}/status") };
    let text = std::fs::read_to_string(path).ok()?;
    let line = text.lines().find(|l| l.starts_with("VmHWM:"))?;
    let kb: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kb * 1024)
}

/// This process's resident set in bytes, from `/proc/self/statm`. Cheap.
pub fn rss_now() -> u64 {
    rss_of(0).unwrap_or(0)
}

/// A process's resident set in bytes from `/proc/<pid>/statm` (`pid` 0: this process). It
/// reads the kernel's counters and does not walk the page tables.
pub fn rss_of(pid: i32) -> Option<u64> {
    let path = if pid == 0 { "/proc/self/statm".to_string() } else { format!("/proc/{pid}/statm") };
    let text = std::fs::read_to_string(path).ok()?;
    let pages: u64 = text.split_whitespace().nth(1)?.parse().ok()?;
    Some(pages * 4096)
}

#[cfg(test)]
mod tests {
    use super::Rollup;

    #[test]
    fn parses_rollup() {
        let t = "00400000-7ffd [rollup]\nRss:  100 kB\nPss:  90 kB\nAnonymous:  80 kB\nAnonHugePages:  2048 kB\n";
        let r = Rollup::parse(t);
        assert_eq!(r.rss, 100 * 1024);
        assert_eq!(r.anon_huge, 2048 * 1024);
        assert!(Rollup::read(0).unwrap().rss > 0);
    }
}
