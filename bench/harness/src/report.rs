//! The result line a workload prints, and timing of measured phases.
use crate::Args;
use crate::perf::{Counters, Sample};
use serde_json::{Map, Value, json};
use std::time::Instant;

/// Which direction of the primary metric is better.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Better {
    Lower,
    Higher,
}

impl Better {
    pub fn name(self) -> &'static str {
        match self {
            Better::Lower => "lower",
            Better::Higher => "higher",
        }
    }
}

/// One measured phase: wall time and counter deltas over all threads.
#[derive(Clone, Debug)]
pub struct Phase {
    pub wall_ns: u64,
    pub counters: Sample,
}

/// Counts one phase. The counters run only between `start` and `stop`, so phases can be
/// interleaved with unmeasured setup.
pub struct Measure<'a> {
    counters: &'a Counters,
    before: Sample,
    t0: Instant,
}

impl<'a> Measure<'a> {
    pub fn start(counters: &'a Counters) -> Measure<'a> {
        let before = counters.read();
        counters.enable();
        Measure { counters, before, t0: Instant::now() }
    }

    pub fn stop(self) -> Phase {
        let wall_ns = self.t0.elapsed().as_nanos() as u64;
        self.counters.disable();
        Phase { wall_ns, counters: self.counters.read().since(&self.before) }
    }
}

/// Builds the `BENCH_RESULT` line.
pub struct Report {
    workload: String,
    params: Map<String, Value>,
    phases: Map<String, Value>,
    metrics: Map<String, Value>,
    primary: Option<Value>,
}

impl Report {
    pub fn new(workload: &str, args: &Args) -> Report {
        let params = args.pairs().iter().map(|(k, v)| (k.clone(), Value::String(v.clone()))).collect();
        Report { workload: workload.to_string(), params, phases: Map::new(), metrics: Map::new(), primary: None }
    }

    /// Records a parameter the workload chose (a default, or a derived value).
    pub fn param(&mut self, name: &str, value: impl Into<Value>) -> &mut Self {
        self.params.insert(name.into(), value.into());
        self
    }

    pub fn phase(&mut self, name: &str, p: &Phase) -> &mut Self {
        let mut v = p.counters.to_json();
        v.as_object_mut().unwrap().insert("wall_ns".into(), p.wall_ns.into());
        self.phases.insert(name.into(), v);
        self
    }

    pub fn metric(&mut self, name: &str, value: impl Into<Value>) -> &mut Self {
        self.metrics.insert(name.into(), value.into());
        self
    }

    /// The one number that ranks allocators on this workload, with its unit in the name
    /// (for example `cycles/op` or `ns/object`).
    pub fn primary(&mut self, name: &str, value: f64, better: Better) -> &mut Self {
        self.primary = Some(json!({ "name": name, "value": value, "better": better.name() }));
        self
    }

    /// Prints the result line. Call it after the measured work, from the main thread.
    pub fn emit(&mut self) {
        let d = bench_alloc::describe();
        let v = json!({
            "workload": self.workload,
            "params": self.params,
            "allocator": d.allocator,
            "mode": d.mode,
            "library": d.library,
            "version": d.version,
            "glibc_rseq_size": d.glibc_rseq_size,
            "vm_hwm": crate::mem::vm_hwm(0),
            "phases": self.phases,
            "metrics": self.metrics,
            "primary": self.primary,
        });
        println!("BENCH_RESULT {v}");
    }
}
