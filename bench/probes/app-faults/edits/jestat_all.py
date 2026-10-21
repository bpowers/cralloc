import os
import sys
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from sub import sub

# Diagnostic: jemalloc's statistics sampler appends every sample, with its time, instead of
# keeping the one with the largest resident set, so that a run can be read under load and at
# rest.
J = "bench/alloc/src/je.rs"
sub(J, """        let mut best_rss = 0;
        loop {
            let sample = stats();
            let rss = sample.iter().find(|(k, _)| *k == "rss").map_or(0, |s| s.1);
            if rss >= best_rss {
                best_rss = rss;
                let text: String = sample.iter().map(|(k, v)| format!("{k}={v} ")).collect();
                let _ = std::fs::write(&path, format!("jemalloc-peak {text}\\n"));
            }
            std::thread::sleep(std::time::Duration::from_millis(ms));
        }""", """        use std::io::Write as _;
        let start = std::time::Instant::now();
        let Ok(mut out) = std::fs::File::create(&path) else {
            return;
        };
        loop {
            let sample = stats();
            let text: String = sample.iter().map(|(k, v)| format!("{k}={v} ")).collect();
            let _ = writeln!(out, "jemalloc t_ms={} {text}", start.elapsed().as_millis());
            std::thread::sleep(std::time::Duration::from_millis(ms));
        }""")
