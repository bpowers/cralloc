use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=gen.py");
    println!("cargo:rerun-if-env-changed=CHAIN_GEN");
    println!("cargo:rerun-if-env-changed=CHAIN_SPLIT");
    println!("cargo:rerun-if-env-changed=CHAIN_CLASS");
    let gen = std::env::var("CHAIN_GEN").unwrap_or_else(|_| "crates/asmalloc/src/front/generated".into());
    for f in ["x86_64_inline/slab.rs", "x86_64/centry.rs"] {
        println!("cargo:rerun-if-changed=../../../{gen}/{f}");
    }
    let out = format!("{}/kernels.rs", std::env::var("OUT_DIR").unwrap());
    let status =
        Command::new("python3").args(["gen.py", &out]).status().expect("python3 is needed to generate kernels");
    assert!(status.success(), "gen.py failed");
}
