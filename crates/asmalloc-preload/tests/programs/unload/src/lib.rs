//! A plugin: a Rust shared library with asmalloc as its global allocator. `host.c` loads it,
//! calls it and unloads it.

#[global_allocator]
static GLOBAL: asmalloc::Asmalloc = asmalloc::Asmalloc;

/// Allocates and frees `n` small blocks, and returns the sum of what they held.
#[unsafe(no_mangle)]
pub extern "C" fn plugin_work(n: usize) -> usize {
    let blocks: Vec<Box<usize>> = (0..n).map(Box::new).collect();
    blocks.iter().map(|b| **b).sum()
}
