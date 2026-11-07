//! The machine as the probes need it, from sysfs: which CPUs to use, printed as shell variables for
//! `run.sh`, and a table for the reader.
//!
//! - `BIG`, a CPU of the fastest kind, and `BIG_NEAR`, another that shares its last-level cache.
//! - `BIG_FAR`, one in another cache domain, if there is one; otherwise the CPU farthest from `BIG`
//!   by number, which on a mesh is as far as a number can say.
//! - `LITTLE`, a CPU of the slowest kind, if the machine has two kinds; `LITTLE_NEAR`, its neighbour.
//! - `DOMAIN`, the CPUs of `BIG`'s domain, and `OTHERS`, up to eight other CPUs of it.
use roofline_machine::parse_cpus;

fn sys(cpu: usize, leaf: &str) -> Option<String> {
    std::fs::read_to_string(format!("/sys/devices/system/cpu/cpu{cpu}/{leaf}")).ok().map(|s| s.trim().to_string())
}

/// The list of CPUs sharing `cpu`'s highest cache index that lists any, whatever its number.
fn domain(cpu: usize) -> Vec<usize> {
    let mut best = vec![cpu];
    for index in 0..8 {
        if let Some(list) = sys(cpu, &format!("cache/index{index}/shared_cpu_list")) {
            if !list.is_empty() {
                best = parse_cpus(&list);
            }
        }
    }
    best
}

fn join(cpus: &[usize]) -> String {
    cpus.iter().map(usize::to_string).collect::<Vec<_>>().join(",")
}

fn main() {
    let online = std::fs::read_to_string("/sys/devices/system/cpu/online").expect("no list of online CPUs");
    let cpus = parse_cpus(online.trim());
    let freq = |c: usize| -> u64 { sys(c, "cpufreq/cpuinfo_max_freq").and_then(|s| s.parse().ok()).unwrap_or(0) };
    let top = cpus.iter().map(|&c| freq(c)).max().unwrap_or(0);
    let bottom = cpus.iter().map(|&c| freq(c)).min().unwrap_or(0);
    // The last CPU of the fastest kind: on a machine with two kinds the low numbers are often the
    // little ones, and CPU 0 takes most interrupts.
    let big = *cpus.iter().rev().find(|&&c| freq(c) == top).unwrap();
    let own = domain(big);
    let near = own.iter().rev().copied().find(|&c| c != big && freq(c) == top);
    let far = cpus
        .iter()
        .copied()
        .find(|c| !own.contains(c) && freq(*c) == top)
        .or_else(|| cpus.iter().copied().find(|&c| c != big && Some(c) != near && freq(c) == top));
    let little = (bottom != top).then(|| *cpus.iter().find(|&&c| freq(c) == bottom).unwrap());
    let little_near = little.and_then(|l| domain(l).into_iter().find(|&c| c != l));
    let others: Vec<usize> = own.iter().rev().copied().filter(|&c| c != big && freq(c) == top).take(8).collect();
    let show = |name: &str, c: Option<usize>| println!("{name}={}", c.map_or(String::new(), |c| c.to_string()));
    println!("BIG={big}");
    show("BIG_NEAR", near);
    show("BIG_FAR", far);
    show("LITTLE", little);
    show("LITTLE_NEAR", little_near);
    println!("DOMAIN={}", join(&own));
    println!("OTHERS={}", join(&others));
    println!("ALL={}", join(&cpus));
    // The table, as comments, so that the output can be sourced.
    let mut seen: Vec<Vec<usize>> = vec![];
    for &c in &cpus {
        let d = domain(c);
        if !seen.contains(&d) {
            let midr = sys(c, "regs/identification/midr_el1").unwrap_or_default();
            println!("# domain {:?}: {} kHz, MIDR {midr}", d, freq(c));
            for index in 0..8 {
                let leaf = |name: &str| sys(c, &format!("cache/index{index}/{name}")).unwrap_or_default();
                if !leaf("size").is_empty() {
                    println!(
                        "#   L{} {}: {}, {} ways, {} sets, line {}, shared {}",
                        leaf("level"),
                        leaf("type"),
                        leaf("size"),
                        leaf("ways_of_associativity"),
                        leaf("number_of_sets"),
                        leaf("coherency_line_size"),
                        leaf("shared_cpu_list")
                    );
                }
            }
            seen.push(d);
        }
    }
}
