use std::process::Command;

fn main() {
    let out = std::env::var("OUT_DIR").unwrap();
    println!("cargo:rerun-if-changed=kernels");
    // The `executable` form's direct `adrp` to `__rseq_offset` needs a copy relocation, which GNU
    // ld makes in a position-dependent executable and refuses in a PIE; lld makes it in both.
    println!("cargo:rustc-link-arg=-no-pie");
    let status = Command::new("python3")
        .args(["kernels/gen.py", &out])
        .status()
        .expect("python3 is needed to generate the kernels");
    assert!(status.success(), "kernels/gen.py failed");
}
