# Servers: first session of the servers suite

Measured 2026-10-20 at the `apps-servers` branch (asmalloc as at `fffde81`), with `benchrun run --suite servers`
(`bench/apps/README.md`, "Servers"): connect-rust's log-ingest service and the order
service on axum and on actix-web, each with every allocator of `bench/allocators.toml` as its
`#[global_allocator]`, driven by `bench/probes/loadgen` over HTTP/1.1. Server on CPUs 0-3,
generator on 8-11, three rounds, allocators in a new random order every round, each piece
(one server, one round) under `scripts/measure --both-l3` with gaps between. No run was
invalid. Raw results: `docs/research/data/servers-held-2026-10-20.jsonl.gz` (the session
reported here) and `servers-free-2026-10-20.jsonl.gz` (the same with idle states left alone).

The day: closed-loop fill and saturation, 6 s at 30% and 6 s at 70% of the slowest
allocator's saturated rate (measured with idle states free: connect 36k and 85k req/s, axum
100k and 235k, actix 120k and 280k), three 2 s bursts at 70% after 3 s silences, then 10 s
idle with the connections closed. Server cores held out of deep idle states by `SCHED_IDLE`
spinners on their SMT siblings (`server-latency.md`).

## Results, idle states held

Medians over three rounds; cycles per request at 70% ranks (± is half the range over
rounds, all under 2.5%); latency in microseconds from when each request was due; memory
in MiB.

| Server | Allocator | cycles/req | p99 at 30% | p99.9 at 70% | sat. req/s | peak MiB | idle MiB |
|---|---|---:|---:|---:|---:|---:|---:|
| connect | mimalloc | 125k | 30 | 34 | 199k | 20.2 | 20.2 |
| connect | tcmalloc-percpu | 127k | 30 | 35 | 194k | 13.5 | 13.5 |
| connect | jemalloc | 131k | 31 | 34 | 189k | 9.7 | 7.9 |
| connect | asmalloc | 131k | 31 | 35 | 189k | 9.8 | 8.7 |
| connect | glibc | 141k | 33 | 36 | 173k | 8.4 | 6.6 |
| connect | snmalloc | 226k | 51 | 56 | 96k | 16.8 | 16.8 |
| axum | jemalloc | 69k | 19 | 27 | 379k | 205 | 205 |
| axum | tcmalloc-percpu | 70k | 19 | 25 | 370k | 212 | 212 |
| axum | mimalloc | 74k | 21 | 33 | 345k | 260 | 260 |
| axum | asmalloc | 79k | 23 | 33 | 343k | 207 | 204 |
| axum | glibc | 85k | 22 | 66 | 291k | 332 | 331 |
| actix | jemalloc | 56k | 19 | 33 | 432k | 206 | 203 |
| actix | tcmalloc-percpu | 57k | 18 | 32 | 429k | 213 | 213 |
| actix | mimalloc | 62k | 22 | 108 | 392k | 268 | 268 |
| actix | asmalloc | 66k | 19 | 53 | 431k | 209 | 206 |
| actix | glibc | 71k | 22 | 252 | 337k | 350 | 348 |

(The other allocators, mimalloc's variants, the control and per-thread tcmalloc, are in
`benchrun report` of the raw results; the control agrees with mimalloc within 3.4%.)

## What these show

- **Holding the idle states works as the latency note found.** With the states free, p99.9
  at 70% was 0.4 to 6.6 ms for every allocator on every server and ranked runtimes and idle
  states; held, it is 25 to 110 us except glibc on actix, and requests that missed their slot
  fell from tens of thousands to almost none.
- **With the cores held, asmalloc does more work per request at a fixed rate, and only then.**
  With the states free, asmalloc was level with jemalloc on cycles per request at 70% (axum
  58.7k against 58.7k, actix 48.2k against 47.8k) and on instructions (actix 89.6k against
  90.4k). Held, every allocator's cycles rose (the spinner shares the core): jemalloc's by
  17%, asmalloc's by 35% (axum) and 37% (actix), 15 to 17% behind jemalloc. The shared core
  is not the whole cause: asmalloc's instructions per request at 70% rose from 89.6k to 111.6k
  on actix while jemalloc's did not move (90.4k, 88.5k); at saturation, where the workers
  never sleep, asmalloc's instructions are level with jemalloc's (81.7k against 83.5k) and so
  is its saturated rate (431k against 432k). So asmalloc does extra work that grows with how
  often the server's threads sleep and wake, which held cores make shorter and more frequent.
  The build measured is asmalloc at `fffde81`; main has since moved maintenance from the
  runtime's park to a timer (`5d355ed`), so measure again on current main before chasing it.
  The kernels never sleep between operations, so they could not show this.
- **snmalloc takes 1.7 to 2 times the cycles per request of every other allocator on
  connect-rust's log service**, in both modes, reproducibly, and is otherwise mid-field.
- **Memory**: the order service holds the last 100k orders, so its resident set is mostly
  live data; jemalloc, asmalloc and tcmalloc keep it at 205 to 213 MiB, mimalloc at 260,
  snmalloc at 350 to 415 and glibc at 330 to 350. No allocator returned memory in 10 s of
  idleness on the order service; on connect, glibc and jemalloc gave back 1.8 MiB, asmalloc
  1.1.

## Cost

Built once per allocator by `build-apps`: the order service in about 20 s per allocator
(1.3 GB shared build directory); connect-rust's server comes with `benchrun build`. A server
run takes about 46 s, so a round of one server over ten allocators about 8 minutes.
