//! User-mode counters on whatever PMU serves a CPU, found in sysfs, so that the same command runs
//! on any aarch64 Linux: `apple_firestorm_pmu` here, `armv8_pmuv3_0` and the like elsewhere.

use std::ffi::c_void;
use std::fs;
use std::path::Path;

extern "C" {
    fn syscall(num: i64, ...) -> i64;
    fn ioctl(fd: i32, req: u64, ...) -> i32;
    fn read(fd: i32, buf: *mut c_void, n: usize) -> isize;
}

const SYS_PERF_EVENT_OPEN: i64 = 241;
const ENABLE: u64 = 0x2400;
const DISABLE: u64 = 0x2401;
const RESET: u64 = 0x2403;
const PMU_DIR: &str = "/sys/bus/event_source/devices";

pub struct Counter {
    fd: i32,
}

impl Counter {
    pub fn open(pmu: u32, config: u64) -> Option<Counter> {
        // struct perf_event_attr: type, size; config; sample_period; sample_type; read_format;
        // flags. TIME_ENABLED | TIME_RUNNING show multiplexing; disabled, exclude_kernel,
        // exclude_hv.
        let mut attr = [0u64; 16];
        attr[0] = (pmu as u64) | (128u64 << 32);
        attr[1] = config;
        attr[4] = 1 | 2;
        attr[5] = 1 | (1 << 5) | (1 << 6);
        let fd = unsafe { syscall(SYS_PERF_EVENT_OPEN, attr.as_ptr(), 0i32, -1i32, -1i32, 0u64) };
        (fd >= 0).then_some(Counter { fd: fd as i32 })
    }

    pub fn start(&self) {
        unsafe {
            ioctl(self.fd, RESET, 0);
            ioctl(self.fd, ENABLE, 0);
        }
    }

    /// The count, or None if the counter was multiplexed.
    pub fn stop(&self) -> Option<u64> {
        let mut v = [0u64; 3];
        unsafe {
            ioctl(self.fd, DISABLE, 0);
            read(self.fd, v.as_mut_ptr().cast(), 24);
        }
        (v[1] == v[2]).then_some(v[0])
    }
}

/// The PMU that serves a CPU: its name, dynamic type, and the codes of the core's cycle and
/// instruction events.
pub struct Pmu {
    pub name: String,
    pub typ: u32,
    pub cycles: u64,
    pub instructions: u64,
}

/// Whether a kernel CPU list such as `2-5,8` holds `cpu`.
fn in_cpu_list(list: &str, cpu: usize) -> bool {
    list.trim().split(',').any(|part| match part.split_once('-') {
        Some((a, b)) => match (a.parse::<usize>(), b.parse::<usize>()) {
            (Ok(a), Ok(b)) => (a..=b).contains(&cpu),
            _ => false,
        },
        None => part.parse::<usize>() == Ok(cpu),
    })
}

/// An event's code from the PMU's `events` directory: `event=0x11` becomes 0x11.
pub fn sysfs_event(pmu: &str, event: &str) -> Option<u64> {
    let text = fs::read_to_string(format!("{PMU_DIR}/{pmu}/events/{event}")).ok()?;
    let code = text.trim().split(',').find_map(|field| field.strip_prefix("event="))?;
    u64::from_str_radix(code.trim_start_matches("0x"), 16).ok()
}

impl Pmu {
    /// The core PMU whose `cpus` file names `cpu` and which offers a cycle and an instruction
    /// event. `RF_PMU` overrides the search.
    pub fn of_cpu(cpu: usize) -> Pmu {
        let forced = std::env::var("RF_PMU").ok();
        let mut names: Vec<String> = fs::read_dir(PMU_DIR)
            .map(|dir| dir.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect())
            .unwrap_or_default();
        names.sort();
        for name in names {
            if forced.as_ref().is_some_and(|f| *f != name) {
                continue;
            }
            let base = Path::new(PMU_DIR).join(&name);
            let cpus = fs::read_to_string(base.join("cpus")).unwrap_or_default();
            if forced.is_none() && !in_cpu_list(&cpus, cpu) {
                continue;
            }
            let cycles = ["cycles", "cpu_cycles"].iter().find_map(|e| sysfs_event(&name, e));
            let instructions =
                ["instructions", "inst_retired"].iter().find_map(|e| sysfs_event(&name, e));
            let typ = fs::read_to_string(base.join("type")).ok().and_then(|t| t.trim().parse().ok());
            if let (Some(cycles), Some(instructions), Some(typ)) = (cycles, instructions, typ) {
                return Pmu { name, typ, cycles, instructions };
            }
        }
        panic!("no PMU in {PMU_DIR} serves CPU {cpu} with cycle and instruction events; set RF_PMU");
    }
}
