//! A port of connect-rust's `echo_bench` and `log_bench` (benches/rpc at c312bb9,
//! Apache-2.0): the two services, their requests, and a pinned tokio runtime.
//!
//! The services and request builders are copied from `src/bin/echo_server.rs`,
//! `src/bin/log_server.rs` and `src/lib.rs` there; the generated code is under `generated/`.

#[rustfmt::skip]
#[path = "generated/connect/mod.rs"]
pub mod connect;
#[rustfmt::skip]
#[path = "generated/buffa/mod.rs"]
pub mod proto;
pub mod store;

use bench_harness::Cpus;
use buffa::Map;
use connectrpc::{RequestContext, Response, ServiceRequest, ServiceResult};
use proto::bench::v1::*;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

/// A multi-threaded tokio runtime with `workers` workers, each pinned to the next CPU of
/// `cpus`. Blocking-pool threads, if any, are pinned the same way.
pub fn runtime(cpus: &Cpus, workers: usize) -> tokio::runtime::Runtime {
    let next = Arc::new(AtomicUsize::new(0));
    let cpus = cpus.clone();
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(workers)
        .enable_all()
        .on_thread_start(move || cpus.pin(next.fetch_add(1, Ordering::Relaxed)))
        .build()
        .expect("tokio runtime")
}

/// echo_bench's request payload: 64 bytes, long enough for a length-delimited string, short
/// enough that copying it does not dominate.
pub const PAYLOAD: &str = "lorem ipsum dolor sit amet, consectetur adipiscing elit sed do e";

/// From echo_server.rs: copy the borrowed string into the owned response.
pub struct EchoImpl;

#[allow(refining_impl_trait)]
impl connect::bench::v1::EchoService for EchoImpl {
    async fn echo(&self, _ctx: RequestContext, req: ServiceRequest<'_, EchoRequest>) -> ServiceResult<EchoResponse> {
        Response::ok(EchoResponse { message: req.message.to_string(), ..Default::default() })
    }
}

/// From log_server.rs: touch every field of every record through the zero-copy view and
/// return aggregate lengths.
pub struct LogIngestImpl;

#[allow(refining_impl_trait)]
impl connect::bench::v1::LogIngestService for LogIngestImpl {
    async fn ingest(
        &self,
        _ctx: RequestContext,
        req: ServiceRequest<'_, LogRequest>,
    ) -> ServiceResult<LogIngestResponse> {
        let mut count = 0i32;
        let mut total_message_bytes = 0i64;
        let mut total_label_bytes = 0i64;
        let mut max_severity = 0i32;
        for rec in req.records.iter() {
            count += 1;
            let sev = rec.severity.to_i32();
            if sev > max_severity {
                max_severity = sev;
            }
            total_message_bytes += rec.message.len() as i64;
            total_message_bytes += rec.service_name.len() as i64;
            total_message_bytes += rec.instance_id.len() as i64;
            total_message_bytes += rec.trace_id.len() as i64;
            total_message_bytes += rec.span_id.len() as i64;
            total_message_bytes += rec.source.file.len() as i64;
            total_message_bytes += rec.source.function.len() as i64;
            let _ = rec.source.line;
            for (k, v) in rec.labels.iter() {
                total_label_bytes += (k.len() + v.len()) as i64;
            }
        }
        Response::ok(LogIngestResponse {
            count,
            total_message_bytes,
            total_label_bytes,
            max_severity,
            ..Default::default()
        })
    }
}

/// From lib.rs: a string-heavy log batch of `n` records (about 15 KB encoded for 50).
pub fn log_request(n: usize) -> LogRequest {
    LogRequest { records: log_records(0, n), ..Default::default() }
}

/// Records `start` to `start + n` of that batch's pattern, numbered on so that they differ.
pub fn log_records(start: usize, n: usize) -> Vec<LogRecord> {
    (start..start + n)
        .map(|i| {
            let mut labels = Map::default();
            for j in 0..6 {
                labels.insert(format!("label-key-{j}"), format!("label-value-{i}-{j}"));
            }
            LogRecord {
                timestamp_nanos: 1_700_000_000_000_000_000 + i as i64,
                service_name: "api-gateway".into(),
                instance_id: format!("instance-{i:04x}"),
                severity: log_record::Severity::SEVERITY_INFO.into(),
                message: format!(
                    "Processing request from client {i}: \
                     GET /api/v1/users?page={}&limit=50 completed in {}ms with status 200 OK",
                    i % 100,
                    42 + i % 200
                ),
                labels,
                trace_id: format!("{:032x}", 0xDEAD_BEEF_0000_0000u64 + i as u64),
                span_id: format!("{:016x}", 0xCAFE_0000u64 + i as u64),
                source: LogSource {
                    file: format!("src/handlers/user_{}.rs", i % 10),
                    line: 42 + (i as i32 % 500),
                    function: format!("handle_get_users_{}", i % 5),
                    ..Default::default()
                }
                .into(),
                ..Default::default()
            }
        })
        .collect()
}
