//! The model from fuzzer bytes, through the C functions.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| asmalloc_fuzz::fuzz(data, asmalloc_fuzz::c_abi));
