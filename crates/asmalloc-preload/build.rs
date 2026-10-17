//! Binds the unwinder's personality name inside the library without exporting it; see
//! `asmalloc_preload_no_unwinding` in `src/lib.rs`.

fn main() {
    println!(
        "cargo:rustc-cdylib-link-arg=-Wl,--defsym=rust_eh_personality=asmalloc_preload_no_unwinding"
    );
    println!("cargo:rerun-if-changed=build.rs");
}
