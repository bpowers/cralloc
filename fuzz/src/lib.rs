//! The body every fuzz target shares: the first byte picks a profile, the rest is the
//! operation stream (four bytes per decision), run against a fresh subject.
//!
//! The allocator is the testkit's `target`, so pointing the testkit at asmalloc points the
//! fuzzers at it too. Setting `ASMALLOC_FUZZ_MUTANT` to a mutant's name (`overlapping`,
//! `short`, ...) wraps the subject in that mutant, which shows the fuzzer finds the bug.
#![feature(cfg_sanitize)]

use core::ffi::c_void;
use std::sync::OnceLock;

use asmalloc_testkit::mutants::Kind;
use asmalloc_testkit::subject::{CFns, SizedFree, Subject, ViaC};
use asmalloc_testkit::{Profile, run_bytes, target};

/// Per-run caps that keep each input fast and the process far below libFuzzer's RSS limit.
const MAX_SIZE: usize = 1 << 20;
const MAX_ALIGN: usize = 1 << 16;
const MAX_LIVE: usize = 16 << 20;

fn mutant() -> Option<Kind> {
    static MUTANT: OnceLock<Option<Kind>> = OnceLock::new();
    *MUTANT.get_or_init(|| {
        let name = std::env::var("ASMALLOC_FUZZ_MUTANT").ok()?;
        let kind = Kind::from_name(&name).unwrap_or_else(|| {
            let names: Vec<_> = Kind::ALL.iter().map(|k| k.name()).collect();
            panic!("ASMALLOC_FUZZ_MUTANT={name} is not one of {names:?}")
        });
        Some(kind)
    })
}

/// Run one input against a fresh subject from `make`, panicking with the failure report.
pub fn fuzz<S: Subject + 'static>(data: &[u8], make: impl Fn() -> S) {
    let Some((&which, rest)) = data.split_first() else {
        return;
    };
    let profile = Profile::ALL[usize::from(which) % Profile::ALL.len()];
    let profile = profile.capped(MAX_SIZE, MAX_ALIGN, MAX_LIVE);
    let result = match mutant() {
        Some(kind) => run_bytes(&kind.wrap(make()), profile, rest),
        None => run_bytes(&make(), profile, rest),
    };
    if let Err(f) = result {
        panic!("{f}");
    }
}

/// The C functions under test. Under AddressSanitizer they are ASan's, whose `aligned_alloc`
/// still enforces C11's rule that the size be a multiple of the alignment (C17 dropped it), so
/// there the size is rounded up, in the matching sized free too. The conformance cases check
/// the C17 rule.
pub fn c_abi() -> ViaC<CFns> {
    if !cfg!(sanitize = "address") {
        return target::c_abi();
    }
    let sized_free = target::C_ABI.sized_free.map(|s| SizedFree {
        free_aligned_sized: free_aligned_sized_c11,
        ..s
    });
    ViaC(CFns {
        aligned_alloc: aligned_alloc_c11,
        sized_free,
        ..target::C_ABI
    })
}

unsafe extern "C" fn aligned_alloc_c11(align: usize, size: usize) -> *mut c_void {
    // SAFETY: same contract; the rounded size is what the block is then freed with.
    unsafe { (target::C_ABI.aligned_alloc)(align, size.next_multiple_of(align.max(1))) }
}

unsafe extern "C" fn free_aligned_sized_c11(p: *mut c_void, align: usize, size: usize) {
    let free = target::C_ABI
        .sized_free
        .expect("only installed when present")
        .free_aligned_sized;
    // SAFETY: same contract, with the size `aligned_alloc_c11` allocated.
    unsafe { free(p, align, size.next_multiple_of(align.max(1))) }
}
