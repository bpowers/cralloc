//! The model from fuzzer bytes, through the C functions.
#![no_main]

use cralloc_testkit::target;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| cralloc_fuzz::fuzz(data, target::c_abi));
