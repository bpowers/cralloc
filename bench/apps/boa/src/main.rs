//! boa, the JavaScript engine, running the V8 benchmark suite (version 7) on one thread, as
//! boa's own benchmark job runs it with the `boa` command, but with a fixed number of runs
//! of each benchmark (harness.js) instead of as many as fit in a second.
//!
//!   boa v8 dir=DIR counts=Name:N,Name:N,...
//!
//! DIR holds base.js and the benchmark files (bench/apps/boa/fetch.sh puts them there).
//! `counts` must name every benchmark of the suite. The engine is built with the features
//! boa_cli 0.22.0 enables by default; the measured phase is what the command does with a
//! script: create a context, evaluate the files, run, and drop the context.

use bench_app::{Driver, fail};
use boa_engine::{Context, Source};

/// In the order the suite's Makefile concatenates them; base.js first.
const FILES: [&str; 9] = [
    "base.js",
    "richards.js",
    "deltablue.js",
    "crypto.js",
    "raytrace.js",
    "earley-boyer.js",
    "regexp.js",
    "splay.js",
    "navier-stokes.js",
];

fn main() {
    let mut d = Driver::start("boa");
    if d.job != "v8" {
        fail("usage: boa v8 dir=DIR counts=Name:N,...");
    }
    let dir = d.args.str("dir", "");
    let counts = parse_counts(&d.args.str("counts", ""));
    if counts.is_empty() {
        fail("counts=Name:N,... is required");
    }
    let sources: Vec<(&str, String)> = FILES
        .iter()
        .map(|&f| {
            let p = format!("{dir}/{f}");
            (f, std::fs::read_to_string(&p).unwrap_or_else(|e| fail(&format!("{p}: {e}; run bench/apps/fetch boa"))))
        })
        .collect();
    let object: Vec<String> = counts.iter().map(|(n, c)| format!("{n:?}: {c}")).collect();
    let call = format!("RunFixed({{{}}})", object.join(", "));

    let ran = d.measure("v8", || -> Result<String, String> {
        let mut ctx = Context::default();
        for (name, src) in &sources {
            ctx.eval(Source::from_bytes(src)).map_err(|e| format!("{name}: {e}"))?;
        }
        ctx.eval(Source::from_bytes(include_str!("../harness.js"))).map_err(|e| format!("harness.js: {e}"))?;
        let v = ctx.eval(Source::from_bytes(&call)).map_err(|e| format!("running the suite: {e}"))?;
        let s = v.to_string(&mut ctx).map_err(|e| format!("result: {e}"))?;
        Ok(s.to_std_string_escaped())
    });
    let ran = ran.unwrap_or_else(|e| fail(&format!("boa: {e}")));
    let mut ran = parse_counts(&ran);
    let mut want = counts.clone();
    ran.sort();
    want.sort();
    if ran != want {
        fail(&format!("the suite ran {ran:?}, not {want:?}"));
    }
    d.report().metric("benchmarks", ran.len());
    d.finish();
}

fn parse_counts(s: &str) -> Vec<(String, u64)> {
    s.split(',')
        .filter(|p| !p.is_empty())
        .map(|p| {
            let (n, c) = p.split_once(':').unwrap_or_else(|| fail(&format!("{p}: not Name:N")));
            (n.to_string(), c.parse().unwrap_or_else(|_| fail(&format!("{p}: not Name:N"))))
        })
        .collect()
}
