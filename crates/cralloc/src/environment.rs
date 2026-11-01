//! The one setting the allocator takes from the environment: `CRALLOC_BACKGROUND`, which
//! turns the background thread's default off (`0` or empty) or, in the preload library, starts
//! it before `main` (anything else).
//!
//! **Which environment.** The one the process was started with, as the kernel keeps it
//! (`/proc/self/environ`, [`Os::environment`]), not the C library's `environ`. A `getenv` on an
//! arbitrary slow path could read strings a concurrent `setenv` in another thread is freeing
//! (musl frees the old ones), and the program cannot order its own calls against one it does
//! not know is made. So a `setenv` in the program does not change the setting; a program that
//! wants a say at run time has the API (`Cralloc::start_background`, `Cralloc::maintain`).
//!
//! **When.** Once per process, at its first slow path (`depot::generation`, beside the cache
//! topology, for the same reason: a file first opened after the program has entered a seccomp
//! sandbox could kill it), and in the preload library also from its constructor, before
//! `main`, if no slow path came first. Both are before a program such as nginx or PostgreSQL
//! writes a process title over its initial environment, which `/proc/self/environ` then
//! shows. The answer is kept in one word ([`Switch`]), zero until read; two threads that read
//! at once each store the answer only into a zero word, so everyone takes the first one
//! stored. A forked child inherits the word with the rest of its parent's memory, and with it
//! the answer its parent read, which is what the child's own initial environment, a copy of
//! the parent's, says. A program run by `exec` starts with a zero word and reads its own.
//!
//! **How.** The entries are `NAME=value`, each ended by a NUL. They are read in pieces into a
//! buffer on the stack, of a size that does not depend on the environment's, so an
//! environment of megabytes takes more reads and no more memory, and nothing is allocated.
//! The name matches only at the start of an entry and may be split across pieces
//! ([`Scan`]). The first entry with the name decides, as `getenv`'s does. A process with no
//! `/proc`, or one whose read fails before the entry, gets the default.
//!
//! **Privileged programs.** A process that is not dumpable at its first slow path (one started
//! from a setuid binary of a user other than root, a setgid binary, or a binary with file
//! capabilities) cannot open its own `/proc/self/environ` (`EACCES`), so it gets the default
//! whatever its environment says: the variable is ignored for it, as `secure_getenv` ignores
//! the environment of such a process. A program that makes itself non-dumpable later
//! (`prctl(PR_SET_DUMPABLE, 0)`) keeps the setting, since the read happened before.

use crate::os::Os;
use core::sync::atomic::AtomicU64;
use core::sync::atomic::Ordering::Relaxed;

/// What the environment asks of the background thread.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Choice {
    /// Unset: the default, on its conditions.
    Default,
    /// `0` or empty: no thread by default.
    Off,
    /// Anything else: a thread at once (the preload library's constructor).
    On,
}

/// The entry's name and its `=`.
const NAME: &[u8] = b"CRALLOC_BACKGROUND=";

/// The bytes of one piece of the environment read: a few reads for a usual environment.
const PIECE: usize = 512;

/// The setting as read, once per process: zero until it is read, then `1 + ` the [`Choice`]'s
/// number.
pub(crate) struct Switch(AtomicU64);

impl Switch {
    /// Not read yet.
    pub(crate) const fn new() -> Switch {
        Switch(AtomicU64::new(0))
    }

    /// The setting, reading the environment through `os` if no thread has yet.
    pub(crate) fn get<O: Os>(&self, os: &O) -> Choice {
        if let Some(choice) = decode(self.0.load(Relaxed)) {
            return choice;
        }
        let read = read(os);
        match self.0.compare_exchange(0, encode(read), Relaxed, Relaxed) {
            Ok(_) => read,
            // Another thread stored first. Both read the same environment unless the program
            // wrote over it in between; the first answer is everyone's.
            Err(stored) => decode(stored).unwrap_or(read),
        }
    }
}

const fn encode(choice: Choice) -> u64 {
    match choice {
        Choice::Default => 1,
        Choice::Off => 2,
        Choice::On => 3,
    }
}

const fn decode(word: u64) -> Option<Choice> {
    match word {
        1 => Some(Choice::Default),
        2 => Some(Choice::Off),
        3 => Some(Choice::On),
        _ => None,
    }
}

/// Reads the setting from the environment the process was started with.
fn read<O: Os>(os: &O) -> Choice {
    let mut piece = [0u8; PIECE];
    let mut scan = Scan::new();
    if os.environment(&mut piece, |bytes| scan.feed(bytes)) {
        scan.end()
    } else {
        Choice::Default
    }
}

/// Where a scan is in the entry it is reading.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum At {
    /// This many bytes of [`NAME`] matched from the start of the entry.
    Name(usize),
    /// In an entry of another name: nothing to do until its NUL.
    Other,
    /// In the value of the entry, so far.
    Value(Value),
}

/// The value of an `CRALLOC_BACKGROUND` entry, so far.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Value {
    Empty,
    Zero,
    Other,
}

impl Value {
    const fn choice(self) -> Choice {
        match self {
            Value::Empty | Value::Zero => Choice::Off,
            Value::Other => Choice::On,
        }
    }
}

/// A scan of the environment's bytes for the first `CRALLOC_BACKGROUND=` entry, fed one piece
/// at a time; the pieces may split an entry, its name or its value anywhere.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Scan {
    at: At,
    found: Option<Choice>,
}

impl Scan {
    /// At the start of the first entry.
    pub(crate) const fn new() -> Scan {
        Scan {
            at: At::Name(0),
            found: None,
        }
    }

    /// Takes the next piece; whether the scan wants more, which it does until it has read a
    /// whole entry of the name.
    pub(crate) fn feed(&mut self, piece: &[u8]) -> bool {
        for &byte in piece {
            self.at = match (self.at, byte) {
                (At::Value(value), 0) => {
                    self.found = Some(value.choice());
                    return false;
                }
                (_, 0) => At::Name(0),
                (At::Name(n), _) if NAME.get(n) == Some(&byte) => {
                    if n + 1 == NAME.len() {
                        At::Value(Value::Empty)
                    } else {
                        At::Name(n + 1)
                    }
                }
                (At::Name(_) | At::Other, _) => At::Other,
                (At::Value(Value::Empty), b'0') => At::Value(Value::Zero),
                (At::Value(_), _) => At::Value(Value::Other),
            };
        }
        true
    }

    /// What the environment asked for, once its last byte has been fed. A last entry with no
    /// NUL ends where the environment does.
    pub(crate) fn end(self) -> Choice {
        match (self.found, self.at) {
            (Some(choice), _) => choice,
            (None, At::Value(value)) => value.choice(),
            (None, _) => Choice::Default,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::os::sim::{SimOs, Standard};
    use std::vec::Vec;

    /// What `getenv` would say: the first entry whose name is the variable's.
    fn reference(environment: &[u8]) -> Choice {
        for entry in environment.split(|&b| b == 0) {
            if let Some(value) = entry.strip_prefix(NAME) {
                return match value {
                    b"" | b"0" => Choice::Off,
                    _ => Choice::On,
                };
            }
        }
        Choice::Default
    }

    /// The scan of `environment` fed in pieces of `size`.
    fn scan_in_pieces(environment: &[u8], size: usize) -> Choice {
        let mut scan = Scan::new();
        for piece in environment.chunks(size) {
            if !scan.feed(piece) {
                break;
            }
        }
        scan.end()
    }

    const CASES: &[(&[u8], Choice)] = &[
        (b"CRALLOC_BACKGROUND=0\0", Choice::Off),
        (b"CRALLOC_BACKGROUND=1\0", Choice::On),
        (b"CRALLOC_BACKGROUND=\0", Choice::Off),
        (b"CRALLOC_BACKGROUND=00\0", Choice::On),
        (b"CRALLOC_BACKGROUND=yes\0", Choice::On),
        (b"PATH=/bin\0HOME=/\0", Choice::Default),
        (b"", Choice::Default),
        (b"\0\0", Choice::Default),
        (b"PATH=/bin\0CRALLOC_BACKGROUND=0\0HOME=/\0", Choice::Off),
        // A longer name with the same prefix, then the name itself.
        (b"CRALLOC_BACKGROUNDS=0\0CRALLOC_BACKGROUND=1\0", Choice::On),
        (b"CRALLOC_BACKGROUND_X=0\0", Choice::Default),
        // A name that ends with the variable's is not it.
        (b"X_CRALLOC_BACKGROUND=0\0", Choice::Default),
        (b"XCRALLOC_BACKGROUND=0\0CRALLOC_BACKGROUND=1\0", Choice::On),
        // The name inside a value is not an entry.
        (b"A=CRALLOC_BACKGROUND=0\0", Choice::Default),
        // A prefix of the name ends its entry.
        (b"CRALLOC\0CRALLOC_BACKGROUND=0\0", Choice::Off),
        // The first entry decides, as `getenv`'s does.
        (b"CRALLOC_BACKGROUND=0\0CRALLOC_BACKGROUND=1\0", Choice::Off),
        // An unterminated last entry ends with the environment.
        (b"PATH=/bin\0CRALLOC_BACKGROUND=0", Choice::Off),
        (b"CRALLOC_BACKGROUND=1", Choice::On),
        (b"CRALLOC_BACKGROUND=", Choice::Off),
        (b"CRALLOC_BACKGROUND", Choice::Default),
    ];

    #[test]
    fn the_scan_agrees_with_getenv_however_the_pieces_fall() {
        for &(environment, expected) in CASES {
            assert_eq!(reference(environment), expected, "{environment:?}");
            for size in 1..=environment.len() + 1 {
                assert_eq!(
                    scan_in_pieces(environment, size),
                    expected,
                    "{environment:?} in pieces of {size}"
                );
            }
        }
    }

    /// The entry at offset 0 and later, split in two at every byte.
    #[test]
    fn an_entry_split_anywhere_is_found() {
        for before in [&b""[..], b"A=1\0", b"LONG_VARIABLE=some value\0"] {
            for value in [&b"0"[..], b"", b"1"] {
                let mut environment = Vec::from(before);
                environment.extend_from_slice(NAME);
                environment.extend_from_slice(value);
                environment.extend_from_slice(b"\0Z=9\0");
                let expected = reference(&environment);
                for cut in 0..=environment.len() {
                    let mut scan = Scan::new();
                    let (a, b) = environment.split_at(cut);
                    if scan.feed(a) {
                        scan.feed(b);
                    }
                    assert_eq!(scan.end(), expected, "{environment:?} cut at {cut}");
                }
            }
        }
    }

    /// An environment of megabytes (under Miri, of tens of pieces) with the entry last, read
    /// through the simulated OS in the pieces the allocator reads.
    #[test]
    fn a_huge_environment_is_read_in_pieces() {
        let entries = crate::scale::under_miri(40_000, 400);
        // Kept in a static, which the simulated OS needs and Miri does not count as leaked.
        static ENVIRONMENT: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
        let environment = ENVIRONMENT.get_or_init(|| {
            let mut environment = Vec::new();
            for i in 0..entries {
                environment.extend_from_slice(std::format!("VARIABLE_{i}=").as_bytes());
                environment.extend_from_slice(&[b'x'; 64]);
                environment.push(0);
            }
            environment.extend_from_slice(b"CRALLOC_BACKGROUND=0\0");
            environment
        });
        assert!(environment.len() > entries * 64);
        assert!(environment.len() > 50 * PIECE);
        let os = SimOs::<Standard>::new();
        os.set_environment(Some(environment));
        assert_eq!(read(&os), Choice::Off);
    }

    #[test]
    fn the_switch_reads_once_and_keeps_the_answer() {
        let os = SimOs::<Standard>::new();
        os.set_environment(Some(b"CRALLOC_BACKGROUND=1\0"));
        let switch = Switch::new();
        assert_eq!(switch.get(&os), Choice::On);
        os.set_environment(Some(b"CRALLOC_BACKGROUND=0\0"));
        assert_eq!(switch.get(&os), Choice::On);
        assert_eq!(os.environment_reads(), 1);
    }

    /// No `/proc`, or a read that fails: the default, read once.
    #[test]
    fn no_environment_is_the_default() {
        let os = SimOs::<Standard>::new();
        let switch = Switch::new();
        assert_eq!(switch.get(&os), Choice::Default);
        assert_eq!(switch.get(&os), Choice::Default);
        assert_eq!(os.environment_reads(), 1);
    }

    /// The real read: this test binary's environment, as `std` sees it at start.
    #[cfg(all(target_os = "linux", target_arch = "x86_64", not(miri)))]
    #[test]
    fn the_kernel_environment_agrees_with_std() {
        let expected = match std::env::var_os("CRALLOC_BACKGROUND") {
            None => Choice::Default,
            Some(v) if v.is_empty() || v == "0" => Choice::Off,
            Some(_) => Choice::On,
        };
        assert_eq!(read(&crate::os::linux::LinuxOs), expected);
    }
}
