//! Test-only access to the constants Lean emits into `lean/generated`, so that each module can
//! pin its own constants to the ones the proved routines were generated with.
//!
//! The constants are read from the emitted source as text, so a test needs neither the routines
//! nor the symbols they link against.

use std::string::String;
use std::vec::Vec;

/// The emitted files that carry the specification's constants.
const SOURCES: [(&str, &str); 2] = [
    (
        "x86_64/slab.rs",
        include_str!("../../../lean/generated/x86_64/slab.rs"),
    ),
    (
        "x86_64_inline/slab.rs",
        include_str!("../../../lean/generated/x86_64_inline/slab.rs"),
    ),
];

/// The value of the emitted constant `name`, as written in `pub(crate) const NAME: TYPE = N;`.
/// Panics if a file lacks it or two files disagree.
pub(crate) fn lean_constant(name: &str) -> u64 {
    let prefix = std::format!("pub(crate) const {name}:");
    let values: Vec<(String, u64)> = SOURCES
        .iter()
        .map(|(file, text)| {
            let line = text
                .lines()
                .find(|line| line.starts_with(&prefix))
                .unwrap_or_else(|| panic!("lean/generated/{file} has no constant {name}"));
            let value = line
                .rsplit('=')
                .next()
                .and_then(|v| v.trim().trim_end_matches(';').trim().parse().ok())
                .unwrap_or_else(|| panic!("cannot read {name} in lean/generated/{file}: {line}"));
            (String::from(*file), value)
        })
        .collect();
    let first = values[0].1;
    for (file, value) in &values {
        assert_eq!(*value, first, "lean/generated/{file} disagrees on {name}");
    }
    first
}
