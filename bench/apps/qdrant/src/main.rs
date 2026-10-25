//! Qdrant 1.20.0's own `main.rs` (written into the build directory by build.rs, with jemalloc
//! taken out as the global allocator and `main` renamed), and the driver that runs it
//! (`driver.rs`).
include!(concat!(env!("OUT_DIR"), "/qdrant_main.rs"));

mod driver;

fn main() {
    driver::main();
}
