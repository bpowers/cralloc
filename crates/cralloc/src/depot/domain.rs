//! Cache domains: which CPUs share a last-level cache, so that the depot can keep the magazines
//! one domain frees for that domain's allocations.
//!
//! A magazine that one L3 filled and another pops costs the popping CPU a fill from the other
//! L3 for each line of the magazine and, mostly, for each object it names: with one depot
//! shared by two L3s, sh8bench took 2.2 times as long and xmalloc-test ran at a third of its
//! speed (`docs/research/magazines.md`, "One depot for the machine ran independent work at half
//! speed"). So the depot has stacks per domain (`super::Depot`), and a slow path uses its
//! CPU's.
//!
//! **Finding them.** Once, from the process's first slow path (`generation::prepare`), and
//! never lazily: a slow path that first opens a file after the program has entered a seccomp
//! sandbox could kill the process. The kernel's list of online CPUs gives the CPUs to cover;
//! the lowest one not yet covered reads the list of CPUs sharing its L3 (`Os::topology`), and
//! every CPU in it gets the next domain number. So there is one read per L3, not per CPU. Any
//! failure (no file, no L3, a refused call, text that does not parse, a CPU missing from its
//! own list) leaves one domain. Domain numbers fold modulo [`MAX_DOMAINS`] on machines with
//! more L3s; folded domains share stacks and work as one.
//!
//! **The table.** One byte per CPU number, the CPU's domain. All zero is valid: every CPU in
//! domain 0. The table is read only after the count is published (`Topology`), and a CPU the
//! table does not name, such as one brought online later, is in domain 0. One thread fills it,
//! the one that claims the work with a compare-and-swap; the others carry on in domain 0 until
//! it publishes, and never wait. A forked child inherits the table as it was; if the fork cut
//! the work off, the claim is never released and the child keeps one domain, which is correct
//! and only slower.

use crate::front::SLAB_COUNT;
use crate::os::{Os, Topology};
use core::sync::atomic::Ordering::{AcqRel, Acquire, Relaxed, Release};
use core::sync::atomic::{AtomicU8, AtomicU64};

/// The most domains the depot keeps apart; machines with more fold them together.
///
/// Under Miri, 4. Miri retags a shared reference by visiting each atomic it covers, and the
/// depot holds stacks for every domain and class, so with 64 domains each call through a
/// `&Depot` cost it milliseconds and the depot's tests ran for hours (`scripts/miri`). Four
/// domains still fold, and still have stacks of their own.
#[cfg(not(miri))]
pub const MAX_DOMAINS: usize = 64;
#[cfg(miri)]
pub const MAX_DOMAINS: usize = 4;

/// The CPU numbers the table names: every CPU with a slab, or under Miri, for the reason
/// [`MAX_DOMAINS`] gives, the first 64, which is more than any simulated machine has.
#[cfg(not(miri))]
const TABLE_CPUS: usize = SLAB_COUNT;
#[cfg(miri)]
const TABLE_CPUS: usize = 64;

/// The bytes of one topology list read: room for any list a machine of [`SLAB_COUNT`] CPUs
/// prints in ranges, and for every CPU of a 64-CPU L3 listed one by one.
const LIST_BYTES: usize = 512;

/// A cache domain's number, below [`MAX_DOMAINS`].
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Domain(u8);

impl Domain {
    /// The domain every CPU is in until the topology is known, and when it cannot be.
    pub const ZERO: Domain = Domain(0);

    /// Domain number `n`, folded below [`MAX_DOMAINS`].
    pub const fn folded(n: usize) -> Domain {
        Domain((n % MAX_DOMAINS) as u8)
    }

    /// The number, below [`MAX_DOMAINS`].
    pub const fn index(self) -> usize {
        self.0 as usize % MAX_DOMAINS
    }
}

/// Where the discovery is: nothing done ([`Topology::NONE`]), claimed and under way, or
/// published with its count of domains.
const CLAIMED: u64 = 1;
/// A published count `n` is stored as `n + PUBLISHED`.
const PUBLISHED: u64 = 2;

/// The CPU table and the state of its discovery.
pub(crate) struct Domains {
    state: AtomicU64,
    of_cpu: [AtomicU8; TABLE_CPUS],
}

impl Domains {
    /// No discovery yet: one domain.
    pub(crate) const fn new() -> Domains {
        Domains {
            state: AtomicU64::new(0),
            of_cpu: [const { AtomicU8::new(0) }; TABLE_CPUS],
        }
    }

    /// How many domains the depot uses: 1 until a discovery that found more is published.
    pub(crate) fn count(&self) -> usize {
        match self.state.load(Acquire) {
            n if n > PUBLISHED => (n - PUBLISHED) as usize,
            _ => 1,
        }
    }

    /// The domain of CPU number `cpu`, or domain 0 if there is no CPU or one domain.
    pub(crate) fn of(&self, cpu: Option<usize>) -> Domain {
        let Some(cpu) = cpu else {
            return Domain::ZERO;
        };
        if self.state.load(Acquire) <= PUBLISHED + 1 {
            return Domain::ZERO;
        }
        match self.of_cpu.get(cpu) {
            Some(byte) => Domain::folded(byte.load(Relaxed) as usize),
            None => Domain::ZERO,
        }
    }

    /// Finds the domains if no thread has started to: see the module documentation.
    pub(crate) fn discover<O: Os>(&self, os: &O) {
        if self.state.load(Relaxed) != 0
            || self
                .state
                .compare_exchange(0, CLAIMED, AcqRel, Relaxed)
                .is_err()
        {
            return;
        }
        let count = self.fill(os).unwrap_or(1);
        self.state.store(count as u64 + PUBLISHED, Release);
    }

    /// Puts each online CPU `c` in domain `c / width`, folded, and publishes the count, in
    /// place of whatever discovery found or is finding (`Depot::split_domains`). Without an
    /// online list, every CPU number with a slab.
    #[cfg(any(test, feature = "internals"))]
    pub(crate) fn split<O: Os>(&self, os: &O, width: usize) {
        let width = width.max(1);
        let mut online = [0u8; LIST_BYTES];
        let mut highest = None;
        if let Some(len) = os.topology(Topology::Online, &mut online) {
            each_cpu(online.get(..len).unwrap_or(&[]), |cpu| {
                if cpu < SLAB_COUNT {
                    highest = highest.max(Some(cpu));
                }
            });
        }
        let highest = highest.unwrap_or(SLAB_COUNT - 1);
        for (cpu, byte) in self.of_cpu.iter().enumerate() {
            byte.store(Domain::folded(cpu / width).0, Relaxed);
        }
        let count = (highest / width + 1).min(MAX_DOMAINS);
        self.state.store(count as u64 + PUBLISHED, Release);
    }

    /// Fills the table from the OS's lists and returns the number of domains, at most
    /// [`MAX_DOMAINS`]; `None` if anything fails, when the table is not to be read.
    fn fill<O: Os>(&self, os: &O) -> Option<usize> {
        let mut online = [0u8; LIST_BYTES];
        let len = os.topology(Topology::Online, &mut online)?;
        let online = online.get(..len)?;
        let mut covered = [0u64; SLAB_COUNT / 64];
        let mut next = 0usize;
        let mut ok = true;
        let parsed = each_cpu(online, |cpu| {
            if !ok || cpu >= SLAB_COUNT || covered[cpu / 64] & (1 << (cpu % 64)) != 0 {
                return;
            }
            let domain = Domain::folded(next);
            next += 1;
            let mut list = [0u8; LIST_BYTES];
            let Some(len) = os.topology(Topology::SharedL3(cpu as u32), &mut list) else {
                ok = false;
                return;
            };
            let mut found_self = false;
            let shared = each_cpu(list.get(..len).unwrap_or(&[]), |other| {
                if other == cpu {
                    found_self = true;
                }
                if other < SLAB_COUNT {
                    covered[other / 64] |= 1 << (other % 64);
                }
                if let Some(byte) = self.of_cpu.get(other) {
                    byte.store(domain.0, Relaxed);
                }
            });
            ok &= shared && found_self;
        });
        (parsed && ok && next > 0).then_some(next.min(MAX_DOMAINS))
    }
}

/// Calls `f` with each CPU number of a list as the kernel prints CPU lists (`0-3,8,10-11`,
/// then a newline). Whether the text was such a list; `f` may have been called for a prefix
/// of a list that was not.
pub(crate) fn each_cpu(text: &[u8], mut f: impl FnMut(usize)) -> bool {
    let text = match text.split_last() {
        Some((b'\n', rest)) => rest,
        _ => text,
    };
    if text.is_empty() {
        return false;
    }
    for part in text.split(|&b| b == b',') {
        let (first, last) = match part.iter().position(|&b| b == b'-') {
            Some(dash) => (number(&part[..dash]), number(&part[dash + 1..])),
            None => (number(part), number(part)),
        };
        let (Some(first), Some(last)) = (first, last) else {
            return false;
        };
        if first > last {
            return false;
        }
        // Past the slab count no CPU has a slab; stop there rather than count up to a huge
        // number a broken file might hold.
        for cpu in first..=last.min(SLAB_COUNT) {
            f(cpu);
        }
    }
    true
}

/// A decimal number of up to six digits.
fn number(digits: &[u8]) -> Option<usize> {
    if digits.is_empty() || digits.len() > 6 {
        return None;
    }
    digits.iter().try_fold(0usize, |n, &d| {
        d.is_ascii_digit().then(|| n * 10 + (d - b'0') as usize)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::vec::Vec;

    fn cpus(text: &str) -> Option<Vec<usize>> {
        let mut out = Vec::new();
        each_cpu(text.as_bytes(), |c| out.push(c)).then_some(out)
    }

    /// The domains found under `topology`: their count, and the domain of each simulated CPU.
    fn found(topology: crate::os::sim::SimTopology) -> (usize, Vec<usize>) {
        let os: crate::os::sim::SimOs = crate::os::sim::SimOs::new();
        os.set_topology(topology);
        let domains = Domains::new();
        assert_eq!(domains.count(), 1, "one domain before discovery");
        domains.discover(&os);
        let of = (0..crate::os::sim::SIM_THREADS)
            .map(|cpu| domains.of(Some(cpu)).index())
            .collect();
        (domains.count(), of)
    }

    #[test]
    fn discovery_gives_each_l3_a_domain_in_order_of_its_lowest_cpu() {
        use crate::os::sim::SimTopology::*;
        let cpus = crate::os::sim::SIM_THREADS;
        // Eight L3s.
        let width = cpus / 8;
        let (count, of) = found(Blocks(width as u32));
        assert_eq!(count, 8.min(MAX_DOMAINS));
        assert!((0..cpus).all(|cpu| of[cpu] == cpu / width % MAX_DOMAINS));
        let (count, of) = found(Stripes(2));
        assert_eq!(count, 2);
        assert!((0..cpus).all(|cpu| of[cpu] == cpu % 2));
        // Every CPU its own L3: as many domains as the table keeps apart.
        let (count, of) = found(Blocks(1));
        assert_eq!(count, MAX_DOMAINS);
        assert!((0..cpus).all(|cpu| of[cpu] == cpu % MAX_DOMAINS));
        // One L3 for all.
        let (count, of) = found(Blocks(cpus as u32));
        assert_eq!(count, 1);
        assert!(of.iter().all(|&d| d == 0));
    }

    #[test]
    fn any_failure_leaves_one_domain() {
        use crate::os::sim::SimTopology::*;
        for topology in [Missing, NoL3, Garbage] {
            let (count, of) = found(topology);
            assert_eq!(count, 1, "{topology:?}");
            assert!(of.iter().all(|&d| d == 0), "{topology:?}");
        }
    }

    #[test]
    fn a_cpu_without_a_slab_or_a_thread_without_a_cpu_is_in_domain_zero() {
        let os: crate::os::sim::SimOs = crate::os::sim::SimOs::new();
        os.set_topology(crate::os::sim::SimTopology::Blocks(4));
        let domains = Domains::new();
        domains.discover(&os);
        assert_eq!(domains.of(None), Domain::ZERO);
        assert_eq!(domains.of(Some(SLAB_COUNT + 5)), Domain::ZERO);
        assert_eq!(domains.of(Some(5)), Domain::folded(1));
        // A second discovery changes nothing.
        os.set_topology(crate::os::sim::SimTopology::Blocks(1));
        domains.discover(&os);
        assert_eq!(
            domains.count(),
            (crate::os::sim::SIM_THREADS / 4).min(MAX_DOMAINS)
        );
    }

    /// What a forked child inherits when the fork cut the discovery off: a claim nobody
    /// holds and a table part written. It keeps one domain and never retries.
    #[test]
    fn an_abandoned_discovery_leaves_one_domain_for_good() {
        let os: crate::os::sim::SimOs = crate::os::sim::SimOs::new();
        os.set_topology(crate::os::sim::SimTopology::Blocks(4));
        let domains = Domains::new();
        domains.state.store(CLAIMED, Release);
        for cpu in 4..12 {
            domains.of_cpu[cpu].store((cpu / 4) as u8, Relaxed);
        }
        domains.discover(&os);
        assert_eq!(domains.count(), 1);
        assert!((0..64).all(|cpu| domains.of(Some(cpu)) == Domain::ZERO));
    }

    /// Lists the kernel does not print but a broken or foreign file might hold never index
    /// past the table.
    #[test]
    fn odd_lists_stay_inside_the_table() {
        assert_eq!(cpus("0-3,2-5"), Some(std::vec![0, 1, 2, 3, 2, 3, 4, 5]));
        assert_eq!(cpus("7,3"), Some(std::vec![7, 3]));
        let beyond = cpus("8190-999999\n").unwrap();
        assert_eq!(beyond, std::vec![8190, 8191, 8192]);
        assert_eq!(cpus("999999"), Some(std::vec![]));
        for bad in ["0-3\n\n", " 0", "0-3 ", "0--3", "0-3-5", "+1", "0x1", "0\0"] {
            assert_eq!(cpus(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn a_split_replaces_what_discovery_found() {
        let os: crate::os::sim::SimOs = crate::os::sim::SimOs::new();
        // Two L3s.
        os.set_topology(crate::os::sim::SimTopology::Blocks(
            crate::os::sim::SIM_THREADS as u32 / 2,
        ));
        let domains = Domains::new();
        domains.discover(&os);
        assert_eq!(domains.count(), 2);
        domains.split(&os, 4);
        assert_eq!(
            domains.count(),
            (crate::os::sim::SIM_THREADS / 4).min(MAX_DOMAINS)
        );
        assert!((0..64).all(|cpu| domains.of(Some(cpu)).index() == cpu / 4 % MAX_DOMAINS));
        // Without an online list, every CPU number with a slab, folded.
        os.set_topology(crate::os::sim::SimTopology::Missing);
        domains.split(&os, 1);
        assert_eq!(domains.count(), MAX_DOMAINS);
        assert_eq!(domains.of(Some(MAX_DOMAINS + 2)).index(), 2);
    }

    #[test]
    fn domain_numbers_fold_beyond_the_most_kept_apart() {
        assert_eq!(Domain::folded(MAX_DOMAINS), Domain::ZERO);
        assert_eq!(Domain::folded(MAX_DOMAINS + 3).index(), 3);
    }

    #[cfg(cralloc_os)]
    #[test]
    #[cfg_attr(miri, ignore)]
    fn this_machine_has_a_topology_the_discovery_reads() {
        use crate::os::linux::LinuxOs;
        let domains = Domains::new();
        domains.discover(&LinuxOs);
        // The distinct L3 lists of the online CPUs, read with the standard library.
        let mut lists = std::collections::BTreeSet::new();
        let online = std::fs::read_to_string("/sys/devices/system/cpu/online").unwrap_or_default();
        each_cpu(online.as_bytes(), |cpu| {
            let path =
                std::format!("/sys/devices/system/cpu/cpu{cpu}/cache/index3/shared_cpu_list");
            if let Ok(list) = std::fs::read_to_string(path) {
                lists.insert(list);
            }
        });
        let expected = lists.len().clamp(1, MAX_DOMAINS);
        assert_eq!(domains.count(), expected);
        // CPU 0 is always in domain 0.
        assert_eq!(domains.of(Some(0)), Domain::ZERO);
    }

    #[test]
    fn lists_parse_as_the_kernel_prints_them() {
        assert_eq!(
            cpus("0-3,8,10-11\n"),
            Some(std::vec![0, 1, 2, 3, 8, 10, 11])
        );
        assert_eq!(cpus("5"), Some(std::vec![5]));
        assert_eq!(cpus("0,2,4\n"), Some(std::vec![0, 2, 4]));
        for bad in [
            "", "\n", "x", "1-", "-1", "3-1", "1,,2", "1 2", "1234567", "0-3,a",
        ] {
            assert_eq!(cpus(bad), None, "{bad:?}");
        }
    }
}
