use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=gen_layout.py");
    let out = format!("{}/layout.rs", std::env::var("OUT_DIR").unwrap());
    let status = Command::new("python3").args(["gen_layout.py", &out]).status().expect("python3 is needed to generate the layout sweep");
    assert!(status.success(), "gen_layout.py failed");
}
