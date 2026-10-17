//! The allocator list, `bench/allocators.toml`.
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Expect {
    pub allocator: Option<String>,
    pub mode: Option<String>,
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
}

impl Allocator {
    /// The build directory name for native binaries with this allocator.
    pub fn build_name(&self) -> Option<String> {
        self.feature.as_ref().map(|f| if f.is_empty() { "glibc".to_string() } else { f.clone() })
    }
}

pub fn load(bench_dir: &Path) -> Result<Vec<Allocator>, String> {
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
                Allocator { name: e.name.clone(), control_of: Some(orig.clone()), ..base.clone() }
            }
            None => Allocator {
                name: e.name.clone(),
                feature: e.feature.clone(),
                preload: e
                    .preload
                    .as_ref()
                    .map(|p| if p.is_empty() { String::new() } else { abs(bench_dir, p).display().to_string() }),
                env: e.env.clone(),
                expect: e.expect.clone(),
                control_of: None,
            },
        };
        out.push(a);
    }
    Ok(out)
}

fn abs(base: &Path, p: &str) -> PathBuf {
    let p = Path::new(p);
    if p.is_absolute() { p.to_path_buf() } else { base.join(p) }
}
