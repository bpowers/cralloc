//! Writes Qdrant's `src/main.rs` at the release, from the checkout cargo made of Qdrant's
//! repository for its library crates, into `$OUT_DIR/qdrant_main.rs`, which `src/main.rs`
//! includes, with three edits and no others:
//!
//! - the two items that make jemalloc the global allocator are left out (the driver's is
//!   bench-alloc's);
//! - its `main` is renamed `qdrant_main`, which the driver's `main` calls;
//! - each `mod NAME;` gets a `#[path]` to its file in the checkout, so that Qdrant's server
//!   modules compile from there, unchanged.
//!
//! Each edit must match exactly once, or the build fails: a release whose `main.rs` differs
//! needs this script read again.
use std::path::{Path, PathBuf};

/// The commit of Qdrant 1.20.0, as Cargo.toml names it for every Qdrant crate.
const REV: &str = "222b2e44ab1c3f17e8246396ead6558e45041df7";

const JEMALLOC_USE: &str = "#[cfg(all(
    not(target_env = \"msvc\"),
    any(target_arch = \"x86_64\", target_arch = \"aarch64\")
))]
use tikv_jemallocator::Jemalloc;
";

const JEMALLOC_GLOBAL: &str = "#[cfg(all(
    not(target_env = \"msvc\"),
    any(target_arch = \"x86_64\", target_arch = \"aarch64\")
))]
#[global_allocator]
static GLOBAL: Jemalloc = Jemalloc;
";

const MAIN: &str = "\nfn main() -> anyhow::Result<()> {\n";

fn main() {
    let src = checkout().join("src");
    let main_rs = src.join("main.rs");
    println!("cargo:rerun-if-changed={}", main_rs.display());
    let text = std::fs::read_to_string(&main_rs).unwrap_or_else(|e| panic!("{}: {e}", main_rs.display()));
    let text = once(&text, JEMALLOC_USE, "");
    let text = once(&text, JEMALLOC_GLOBAL, "");
    let text = once(&text, MAIN, "\npub fn qdrant_main() -> anyhow::Result<()> {\n");
    let mut out = String::new();
    for line in text.lines() {
        match line.strip_prefix("mod ").and_then(|l| l.strip_suffix(';')) {
            Some(name) => {
                let dir = src.join(name).join("mod.rs");
                let file = if dir.is_file() { dir } else { src.join(format!("{name}.rs")) };
                assert!(file.is_file(), "no file for `mod {name};` in {}", src.display());
                out.push_str(&format!("#[path = {:?}]\n{line}\n", file.display().to_string()));
            }
            None => {
                out.push_str(line);
                out.push('\n');
            }
        }
    }
    let dest = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("qdrant_main.rs");
    std::fs::write(&dest, out).unwrap_or_else(|e| panic!("{}: {e}", dest.display()));
}

fn once(text: &str, from: &str, to: &str) -> String {
    let n = text.matches(from).count();
    assert_eq!(n, 1, "Qdrant's main.rs has {n} copies of {from:?}, not one");
    text.replacen(from, to, 1)
}

/// Cargo's checkout of Qdrant's repository at `REV`: `$CARGO_HOME/git/checkouts/<repo>/<short
/// commit>`, where the library crates this package depends on are built from.
fn checkout() -> PathBuf {
    let home = std::env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| Path::new(&h).join(".cargo")))
        .expect("CARGO_HOME or HOME");
    let checkouts = home.join("git/checkouts");
    let short = &REV[..7];
    let found: Vec<PathBuf> = std::fs::read_dir(&checkouts)
        .unwrap_or_else(|e| panic!("{}: {e}", checkouts.display()))
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().starts_with("qdrant-"))
        .map(|e| e.path().join(short))
        .filter(|p| is_qdrant(p))
        .collect();
    match found.as_slice() {
        [one] => one.clone(),
        [] => panic!("no checkout of Qdrant at {short} under {}", checkouts.display()),
        many => panic!("several checkouts of Qdrant at {short}: {many:?}"),
    }
}

fn is_qdrant(dir: &Path) -> bool {
    std::fs::read_to_string(dir.join("Cargo.toml"))
        .is_ok_and(|m| m.contains("name = \"qdrant\"\nversion = \"1.20.0\"") && dir.join("src/main.rs").is_file())
}
