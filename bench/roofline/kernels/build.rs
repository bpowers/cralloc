use std::process::Command;

fn main() {
    for (script, file) in [("gen_layout.py", "layout.rs"), ("gen_routines.py", "routines.rs")] {
        println!("cargo:rerun-if-changed={script}");
        let out = format!("{}/{file}", std::env::var("OUT_DIR").unwrap());
        let status = Command::new("python3").args([script, &out]).status().expect("python3 is needed to generate kernels");
        assert!(status.success(), "{script} failed");
    }
}
