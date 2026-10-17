//! The model from fuzzer bytes, through the `Allocator` trait (zero sizes, any size the block
//! fits on free and resize, alignment changes on resize).
#![no_main]

use asmalloc_testkit::target;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| asmalloc_fuzz::fuzz(data, target::allocator));
