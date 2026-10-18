//! The allocator list, `bench/allocators.toml`.
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Expect {
    pub allocator: Option<String>,
    pub mode: Option<String>,
    /// Where a native run's allocator code must be: "executable", or the file name of a
    /// shared library. A preload run must report the preloaded library itself.
    pub library: Option<String>,
    /// The version the allocator must report at run time, where two builds of one
    /// allocator differ in version (third_party's mimalloc and the `mimalloc` crate's).
    pub version: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    name: String,
    feature: Option<String>,
    preload: Option<String>,
    #[serde(default)]
    env: BTreeMap<String, String>,
    #[serde(default)]
    expect: Expect,
    same_as: Option<String>,
    preload_build: Option<String>,
    build: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    allocator: Vec<Entry>,
}

/// One allocator configuration, resolved.
#[derive(Clone, Debug)]
pub struct Allocator {
    pub name: String,
    /// bench-alloc feature for native builds (`Some("")` is the System allocator); `None`
    /// means the allocator is not run natively.
    pub feature: Option<String>,
    /// Absolute path of the LD_PRELOAD library (`Some("")` for none: glibc); `None` means
    /// the allocator is not run in preload mode.
    pub preload: Option<String>,
    pub env: BTreeMap<String, String>,
    pub expect: Expect,
    /// For a control entry, the allocator it copies.
    pub control_of: Option<String>,
    /// Appended to the native build directory name. A control gets its own build of the
    /// same source, so that it also tests that two builds of one allocator measure alike.
    pub build_suffix: String,
    /// A package of the repository's root workspace that `benchrun build` builds in release
    /// mode to make the preload library (asmalloc's, which lives in this checkout).
    pub preload_build: Option<String>,
    /// The native build directory under target/, for variants of one feature built by hand
    /// (with different allocator build-time knobs); `benchrun build` does not make them.
    pub build: Option<String>,
    /// A code-layout variant of the workload builds (`benchrun layout`, see `layout.rs`):
    /// the same sources and work, placed at other addresses. `None` is the normal build.
    pub layout: Option<String>,
}

impl Allocator {
    /// The build directory name for native binaries with this allocator.
    pub fn build_name(&self) -> Option<String> {
        let base = match &self.build {
            Some(b) => b.clone(),
            None => {
                let f = self.feature.as_ref()?;
                format!("{}{}", if f.is_empty() { "glibc" } else { f.as_str() }, self.build_suffix)
            }
        };
        Some(self.with_layout(base))
    }

    /// The build directory of the Rust programs a preload run starts: the glibc build, in
    /// this entry's layout.
    pub fn preload_build_name(&self) -> String {
        self.with_layout("glibc".into())
    }

    fn with_layout(&self, base: String) -> String {
        match &self.layout {
            Some(l) => format!("{base}-layout-{l}"),
            None => base,
        }
    }
}

/// Reads `allocators.toml` from `bench_dir`. A `preload` path may start with `{baselines}/`,
/// the shared baselines directory; other relative paths are relative to `bench_dir`.
pub fn load(bench_dir: &Path, baselines: &Path) -> Result<Vec<Allocator>, String> {
    let path = bench_dir.join("allocators.toml");
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let file: File = toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut out: Vec<Allocator> = Vec::new();
    for e in &file.allocator {
        if out.iter().any(|a| a.name == e.name) {
            return Err(format!("allocator {} is listed twice", e.name));
        }
        let a = match &e.same_as {
            Some(orig) => {
                let base = out
                    .iter()
                    .find(|a| &a.name == orig)
                    .ok_or_else(|| format!("{}: same_as {orig} must name an earlier entry", e.name))?;
                Allocator {
                    name: e.name.clone(),
                    control_of: Some(orig.clone()),
                    build_suffix: "-ctl".into(),
                    ..base.clone()
                }
            }
            None => Allocator {
                name: e.name.clone(),
                feature: e.feature.clone(),
                preload: e.preload.as_ref().map(|p| {
                    if p.is_empty() { String::new() } else { resolve(bench_dir, baselines, p).display().to_string() }
                }),
                env: e.env.clone(),
                expect: e.expect.clone(),
                control_of: None,
                build_suffix: String::new(),
                preload_build: e.preload_build.clone(),
                build: e.build.clone(),
                layout: None,
            },
        };
        out.push(a);
    }
    Ok(out)
}

fn resolve(bench_dir: &Path, baselines: &Path, p: &str) -> PathBuf {
    if let Some(rest) = p.strip_prefix("{baselines}/") {
        return baselines.join(rest);
    }
    let p = Path::new(p);
    if p.is_absolute() { p.to_path_buf() } else { bench_dir.join(p) }
}
