//! Binds the unwinder's personality name inside the library without exporting it (see
//! `asmalloc_preload_no_unwinding` in `src/lib.rs`), and marks the library never to be unloaded:
//! a thread's `rseq_cs` may point into its routines until the thread is next interrupted, and
//! the heap's blocks outlive any `dlclose` (docs/design.md, "Process lifecycle").

fn main() {
    println!(
        "cargo:rustc-cdylib-link-arg=-Wl,--defsym=rust_eh_personality=asmalloc_preload_no_unwinding"
    );
    println!("cargo:rustc-cdylib-link-arg=-Wl,-z,nodelete");
    println!("cargo:rerun-if-changed=build.rs");
}
