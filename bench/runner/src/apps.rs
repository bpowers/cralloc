//! Builds the programs of the apps suite (bench/apps), once per allocator.
//!
//! Each program is a cargo workspace of its own, with its own lock file, whose driver links
//! bench-alloc and forwards its allocator features. Building it for every allocator from
//! scratch would compile the whole program each time. Instead every build of a program
//! shares one cargo build directory (`<apps data>/build/<program>`): the program's crates do
//! not depend on bench-alloc, so they compile once and every later build reuses them, and
//! each allocator's build compiles only bench-alloc, the driver crates and its allocator,
//! then links, with the program's own release profile (LTO included). The binary lands in a
//! target directory of its own per allocator and checkout (`run::app_target`). The result is
//! the binary a from-scratch build with that feature makes.
//!
//! Builds of one program from several checkouts share the build directory too; cargo locks
//! it, so they take turns.
use crate::paths::Paths;
use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;
use std::time::Instant;

/// The allocator crates whose versions decide what an allocator is. A program's lock file
/// must pin each to the version bench/Cargo.lock pins, or the program would measure another
/// build of the allocator than the workloads do.
const ALLOCATOR_CRATES: &[&str] =
    &["tikv-jemallocator", "tikv-jemalloc-sys", "mimalloc", "libmimalloc-sys", "snmalloc-rs", "snmalloc-sys"];

/// Each package's versions in a Cargo.lock.
fn locked_versions(lock: &Path) -> Result<BTreeMap<String, Vec<String>>, String> {
    #[derive(serde::Deserialize)]
    struct Lock {
        package: Vec<Package>,
    }
    #[derive(serde::Deserialize)]
    struct Package {
        name: String,
        version: String,
        source: Option<String>,
    }
    let text = std::fs::read_to_string(lock).map_err(|e| format!("{}: {e}", lock.display()))?;
    let l: Lock = toml::from_str(&text).map_err(|e| format!("{}: {e}", lock.display()))?;
    let mut m: BTreeMap<String, Vec<String>> = BTreeMap::new();
    // Only crates from a registry or git: a program may put an empty crate of its own in an
    // allocator crate's place (Meilisearch's jemalloc, `apps/meilisearch/no-jemalloc`).
    for p in l.package.into_iter().filter(|p| p.source.is_some()) {
        m.entry(p.name).or_default().push(p.version);
    }
    Ok(m)
}

/// Checks that `program`'s lock file pins every allocator crate as bench/Cargo.lock does.
pub fn check_lock(p: &Paths, program: &str) -> Result<(), String> {
    let bench = locked_versions(&p.bench.join("Cargo.lock"))?;
    let app_lock = p.bench.join("apps").join(program).join("Cargo.lock");
    let app = locked_versions(&app_lock)?;
    let mut wrong = Vec::new();
    for c in ALLOCATOR_CRATES {
        let want = bench.get(*c);
        let got = app.get(*c);
        if want != got {
            wrong.push(format!("{c}: bench/Cargo.lock has {want:?}, {} has {got:?}", app_lock.display()));
        }
    }
    if wrong.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "{program}: its lock file pins other allocator versions than the workloads'; set them with \
             `cargo update -p NAME --precise VERSION` in bench/apps/{program}:\n  {}",
            wrong.join("\n  ")
        ))
    }
}

/// What one build took.
pub struct Built {
    pub build: String,
    /// Wall time; on a shared machine it depends on what else runs.
    pub seconds: f64,
    /// CPU time of every process the build ran (user and system), which does not.
    pub cpu_seconds: f64,
    pub binary_bytes: u64,
}

/// Builds `program` with bench-alloc's `feature` ("" for glibc) into the target directory of
/// `build`, sharing the program's build directory.
pub fn build(p: &Paths, program: &str, build: &str, feature: &str, cpus: &str) -> Result<Built, String> {
    let dir = p.bench.join("apps").join(program);
    let target = crate::run::app_target(&p.bench, build, program);
    let mut c = Command::new("nice");
    c.args(["taskset", "-c", cpus, "cargo", "build", "--release", "--locked"]);
    c.arg("--target-dir").arg(&target);
    if !feature.is_empty() {
        c.args(["--features", feature]);
    }
    c.env("CARGO_BUILD_BUILD_DIR", p.apps_data.join("build").join(program));
    // Flags from the environment would change every crate's fingerprint, and the program
    // would no longer be built with its own profile alone.
    c.env_remove("RUSTFLAGS").env_remove("CARGO_ENCODED_RUSTFLAGS");
    // Nor does a layout's padding (`benchrun layout`), which builds the workloads only.
    c.env_remove("BENCH_LAYOUT_PAD");
    c.env("BENCH_BASELINES", &p.baselines);
    if let Some(path) = tools_path(p, program)? {
        c.env("PATH", path);
    }
    c.current_dir(&dir);
    let t0 = Instant::now();
    let cpu0 = children_cpu_seconds();
    let st = c.status().map_err(|e| format!("cargo: {e}"))?;
    if !st.success() {
        return Err(format!("{program}: build for {build} failed"));
    }
    let bin = crate::run::app_bin(&p.bench, build, program);
    let binary_bytes = std::fs::metadata(&bin).map_err(|e| format!("{}: {e}", bin.display()))?.len();
    Ok(Built {
        build: build.to_string(),
        seconds: t0.elapsed().as_secs_f64(),
        cpu_seconds: children_cpu_seconds() - cpu0,
        binary_bytes,
    })
}

/// The servers' load generator, built once with glibc from its own lock file: it is never the
/// program under test.
pub fn loadgen_bin(bench: &Path) -> std::path::PathBuf {
    bench.join("target/loadgen/release/loadgen")
}

pub fn build_loadgen(p: &Paths, cpus: &str) -> Result<(), String> {
    build_glibc(&p.bench.join("probes/loadgen"), &p.bench.join("target/loadgen"), cpus)
        .map_err(|e| format!("the load generator's build failed: {e}"))
}

/// A proxy job's upstream binary: the generator's workspace's, or its own workspace's.
pub fn upstream_bin(bench: &Path, u: &crate::suite::Upstream) -> std::path::PathBuf {
    match u.workspace {
        None => bench.join("target/loadgen/release").join(u.bin),
        Some(_) => bench.join("target/upstream").join(u.bin).join("release").join(u.bin),
    }
}

/// Builds an upstream that has a workspace of its own (a Router job's subgraphs), with glibc
/// from its own lock file: like the generator, it is never the program under test. Its
/// build tools come from the inputs of `program`, as the program's do.
pub fn build_upstream(p: &Paths, u: &crate::suite::Upstream, program: &str, cpus: &str) -> Result<(), String> {
    let Some(ws) = u.workspace else { return Ok(()) };
    let target = p.bench.join("target/upstream").join(u.bin);
    let mut c = cargo_glibc(&p.bench.join(ws), &target, cpus);
    if let Some(path) = tools_path(p, program)? {
        c.env("PATH", path);
    }
    let st = c.status().map_err(|e| format!("cargo: {e}"))?;
    if st.success() { Ok(()) } else { Err(format!("the upstream {}'s build failed", u.bin)) }
}

fn build_glibc(dir: &Path, target: &Path, cpus: &str) -> Result<(), String> {
    let st = cargo_glibc(dir, target, cpus).status().map_err(|e| format!("cargo: {e}"))?;
    if st.success() { Ok(()) } else { Err(format!("cargo build in {} failed", dir.display())) }
}

fn cargo_glibc(dir: &Path, target: &Path, cpus: &str) -> Command {
    let mut c = Command::new("nice");
    c.args(["taskset", "-c", cpus, "cargo", "build", "--release", "--locked", "--target-dir"])
        .arg(target)
        .env_remove("RUSTFLAGS")
        .env_remove("CARGO_ENCODED_RUSTFLAGS")
        .current_dir(dir);
    c
}

/// PATH with the build tools a program's fetch script provides (Sōzu's protoc) first, when it
/// provides any.
fn tools_path(p: &Paths, program: &str) -> Result<Option<std::ffi::OsString>, String> {
    let tools = p.apps_data.join("inputs").join(program).join("bin");
    if !tools.is_dir() {
        return Ok(None);
    }
    let path = std::env::var_os("PATH").unwrap_or_default();
    std::env::join_paths(std::iter::once(tools).chain(std::env::split_paths(&path)))
        .map(Some)
        .map_err(|e| format!("PATH: {e}"))
}

/// User and system time of this process's waited-for descendants.
fn children_cpu_seconds() -> f64 {
    // SAFETY: rusage is plain data that getrusage fills.
    let mut u: libc::rusage = unsafe { std::mem::zeroed() };
    // SAFETY: a valid pointer to a rusage.
    unsafe { libc::getrusage(libc::RUSAGE_CHILDREN, &mut u) };
    let t = |v: libc::timeval| v.tv_sec as f64 + v.tv_usec as f64 / 1e6;
    t(u.ru_utime) + t(u.ru_stime)
}

/// Bytes under `dir`, as `du` counts them (allocated blocks); 0 if it does not exist.
pub fn disk_use(dir: &Path) -> u64 {
    Command::new("du")
        .args(["-s", "--block-size=1"])
        .arg(dir)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok()?.split_whitespace().next()?.parse().ok())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use crate::paths::Paths;

    fn programs() -> Vec<&'static str> {
        let mut v: Vec<&'static str> = crate::suite::apps()
            .iter()
            .chain(&crate::suite::servers())
            .chain(&crate::suite::servers_large())
            .filter_map(|j| j.program.app())
            .collect();
        v.sort();
        v.dedup();
        v
    }

    #[test]
    fn every_program_pins_the_workloads_allocator_versions() {
        let p = Paths::new();
        for program in programs() {
            super::check_lock(&p, program).unwrap();
        }
    }

    /// `build-apps` passes each allocator's bench-alloc feature to the program's driver, which
    /// must forward it under the same name.
    #[test]
    fn every_program_forwards_every_allocator_feature() {
        let p = Paths::new();
        let allocators = crate::config::load(&p.bench, &p.baselines).unwrap();
        for program in programs() {
            let path = p.bench.join("apps").join(program).join("Cargo.toml");
            let manifest: toml::Table = toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
            let features = manifest.get("features").and_then(|f| f.as_table()).cloned().unwrap_or_default();
            for f in allocators.iter().filter_map(|a| a.feature.as_deref()).filter(|f| !f.is_empty()) {
                let want = toml::Value::Array(vec![toml::Value::String(format!("bench-alloc/{f}"))]);
                assert_eq!(features.get(f), Some(&want), "{}: feature {f}", path.display());
            }
        }
    }
}
