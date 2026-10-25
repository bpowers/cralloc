# Apollo Router

The Apollo Router 2.19.0, the GraphQL federation router, as the `router graphql` job of the
servers suite. Its release binary ships with jemalloc.

- **Driver** (`src/main.rs`): runs `apollo_router::main`, the whole of the `router` binary,
  in a child process (the Router reads its own command line), with the supergraph in
  `supergraph.graphql` and a configuration the driver writes. The Router's
  `global-allocator` feature is off, so the suite's allocator is the global allocator;
  `no-jemalloc-sys/` takes the place of tikv-jemalloc-sys 0.7, which the Router links but
  uses only with that feature. Before it prints `BENCH_LISTEN`, and again after the load, the
  driver sends every distinct query of the load and requires data and no GraphQL errors.
- **Subgraphs** (`subgraphs/`): Apollo's federation demo (accounts, inventory, products,
  reviews) with a catalogue of 10,000 products, written with async-graphql, all four in one
  process that the runner starts as the job's upstream on CPUs 12-14. Built with glibc, never
  the program under test. It remembers each answer by request body, so after the first pass
  through the load every fetch is a lookup: the Router is what is measured.
- **Load** (`fetch.sh`): 20,000 GraphQL POSTs in turn, three in four a page of products with
  their inventory, reviews and authors (four subgraphs, three fetches deep), the rest two
  queries that start from the accounts subgraph. Answers are 10 to 40 KB of JSON.
- **Configuration**: the Router's defaults (telemetry, query-plan cache, traffic shaping)
  but for the listen address, the subgraph URLs, no health check, homepage or sandbox, and
  idle client connections kept for 10 minutes instead of 10 s, since the load generator
  counts a closed connection as a broken run.

`fetch.sh` also provides `protoc`, which the Router's build needs. `docs/research/servers-router.md`
has the measurements.
