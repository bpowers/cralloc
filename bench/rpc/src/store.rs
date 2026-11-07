//! A log store over connect-rust's benchmark services, for a server that streams while it
//! holds a large heap: `LogIngestService.Ingest` keeps every record it is sent (as an owned
//! `LogRecord`) and drops the oldest past `keep`; `BenchService.ServerStream` tails the store,
//! streaming the `response_count` most recent records back, each turned into a
//! `BenchResponse` as a service turns its stored model into its API's messages.
//!
//! Written for the suite (connect-rust has no stateful service); the protocol, the generated
//! code and the record shape are connect-rust's. The store is sharded by arrival so that
//! ingest and tail on different workers rarely meet on one lock, and starts with `keep`
//! records already in it (`log_record`, the records the request generator writes, numbered from 0), so
//! that a run measures the full heap from its first request.
use crate::connect::bench::v1::{BenchService, LogIngestService};
use crate::proto::bench::v1::*;
use buffa::view::MessageView;
use connectrpc::{ConnectError, InboundStream, RequestContext, Response, ServiceRequest, ServiceResult, ServiceStream};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

const SHARDS: usize = 64;

pub struct Store {
    shards: Vec<Mutex<VecDeque<Arc<LogRecord>>>>,
    next: AtomicU64,
    per_shard: usize,
}

impl Store {
    /// A store that keeps the latest `keep` records, holding `keep` already.
    pub fn new(keep: usize) -> Store {
        let per_shard = keep.div_ceil(SHARDS).max(1);
        let s = Store {
            shards: (0..SHARDS).map(|_| Mutex::new(VecDeque::with_capacity(per_shard + 1))).collect(),
            next: AtomicU64::new(0),
            per_shard,
        };
        for i in 0..keep {
            s.push(Arc::new(log_record(i)));
        }
        s
    }

    fn push(&self, r: Arc<LogRecord>) {
        let n = self.next.fetch_add(1, Ordering::Relaxed) as usize;
        let mut shard = self.shards[n % SHARDS].lock().unwrap();
        shard.push_back(r);
        if shard.len() > self.per_shard {
            shard.pop_front();
        }
    }

    /// About the `n` most recent records, newest first.
    fn tail(&self, n: usize) -> Vec<Arc<LogRecord>> {
        let last = self.next.load(Ordering::Relaxed) as usize;
        let mut out = Vec::with_capacity(n);
        for k in 0..n.min(last) {
            let i = last - 1 - k;
            let shard = self.shards[i % SHARDS].lock().unwrap();
            // The record `i` is the shard's `(last - 1 - i) / SHARDS`-th from its end, if the
            // shard still has it (an ingest may have moved on meanwhile).
            let back = k / SHARDS;
            if back < shard.len() {
                out.push(shard[shard.len() - 1 - back].clone());
            }
        }
        out
    }
}

/// The record a stored record becomes in a tail's response.
fn response(r: &LogRecord) -> BenchResponse {
    let source = r.source.as_option();
    let payload = Payload {
        id: source.map_or(0, |s| s.line),
        timestamp_nanos: r.timestamp_nanos,
        trace_id: u64::from_str_radix(&r.span_id, 16).unwrap_or(0),
        name: r.service_name.clone(),
        description: r.message.clone(),
        region: r.instance_id.clone(),
        data: r.trace_id.as_bytes().to_vec(),
        status: Status::STATUS_ACTIVE.into(),
        metadata: Metadata {
            request_id: r.trace_id.clone(),
            user_agent: source.map_or_else(String::new, |s| s.function.clone()),
            created_at: r.timestamp_nanos,
            headers: r.labels.clone(),
            ..Default::default()
        }
        .into(),
        tags: r.labels.values().cloned().collect(),
        attributes: r.labels.clone(),
        ..Default::default()
    };
    BenchResponse { payload: payload.into(), ..Default::default() }
}

/// The log-ingest service, keeping what it is sent.
pub struct StoreIngest(pub Arc<Store>);

#[allow(refining_impl_trait)]
impl LogIngestService for StoreIngest {
    async fn ingest(
        &self,
        _ctx: RequestContext,
        req: ServiceRequest<'_, LogRequest>,
    ) -> ServiceResult<LogIngestResponse> {
        let mut count = 0i32;
        let mut total_message_bytes = 0i64;
        for rec in req.records.iter() {
            let owned = rec.to_owned_message().map_err(|e| ConnectError::invalid_argument(e.to_string()))?;
            count += 1;
            total_message_bytes += owned.message.len() as i64;
            self.0.push(Arc::new(owned));
        }
        Response::ok(LogIngestResponse { count, total_message_bytes, ..Default::default() })
    }
}

/// The bench service, of which only `ServerStream` (the tail) is served.
pub struct StoreTail(pub Arc<Store>);

#[allow(refining_impl_trait)]
impl BenchService for StoreTail {
    async fn unary(
        &self,
        _ctx: RequestContext,
        _req: ServiceRequest<'_, BenchRequest>,
    ) -> ServiceResult<BenchResponse> {
        Err(ConnectError::unimplemented("only ServerStream"))
    }

    async fn server_stream(
        &self,
        _ctx: RequestContext,
        req: ServiceRequest<'_, BenchRequest>,
    ) -> ServiceResult<ServiceStream<BenchResponse>> {
        let n = req.response_count.clamp(0, 10_000) as usize;
        let records = self.0.tail(n);
        let s: ServiceStream<BenchResponse> =
            Box::pin(futures::stream::iter(records.into_iter().map(|r| Ok(response(&r)))));
        Response::ok(s)
    }

    async fn client_stream(
        &self,
        _ctx: RequestContext,
        _req: InboundStream<BenchRequest>,
    ) -> ServiceResult<BenchResponse> {
        Err(ConnectError::unimplemented("only ServerStream"))
    }

    async fn bidi_stream(
        &self,
        _ctx: RequestContext,
        _req: InboundStream<BenchRequest>,
    ) -> ServiceResult<ServiceStream<BenchResponse>> {
        Err(ConnectError::unimplemented("only ServerStream"))
    }

    async fn log_unary(
        &self,
        _ctx: RequestContext,
        _req: ServiceRequest<'_, LogRequest>,
    ) -> ServiceResult<LogResponse> {
        Err(ConnectError::unimplemented("only ServerStream"))
    }

    async fn log_unary_owned(
        &self,
        _ctx: RequestContext,
        _req: ServiceRequest<'_, LogRequest>,
    ) -> ServiceResult<LogResponse> {
        Err(ConnectError::unimplemented("only ServerStream"))
    }
}

/// Record `i` of connect-rust's `log_request` pattern, numbered across batches so that stored
/// records differ; `loadgen/connect_store_requests.py` writes the same ones.
pub fn log_record(i: usize) -> LogRecord {
    crate::log_records(i, 1).pop().expect("one record")
}
