//! The model from fuzzer bytes, through `GlobalAlloc`.
#![no_main]

use cralloc_testkit::target;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| cralloc_fuzz::fuzz(data, target::global_alloc));
