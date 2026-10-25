# SurrealDB

SurrealDB 3.3.4 with its in-memory engine, as the `surrealdb mem` and `surrealdb mem 1.5M`
jobs of the large-heap suite (`--suite servers-large`, `servers-thread`; run with
`--mem-cap 10G`, 12G for 1.5M). Its release binary ships with jemalloc.

- **Driver** (`src/main.rs`): runs `surrealdb_server::init`, the whole of the `surreal`
  binary, in a child process (SurrealDB reads its own command line), as
  `surreal start memory` on a loopback port, without authentication (with it, every request
  would verify the root password by hashing), warnings only. Before it prints `BENCH_LISTEN`
  it fills table `user` through the HTTP API with 750,000 records (1,500,000 for the second
  job): a name, an e-mail address with a unique index, an age, a country with an index,
  three tags and a paragraph of text, about 70 s for 750k. It checks one distinct query in
  97 of the load before and after it.
- **Build**: `surrealdb-server` and `surrealdb-core` from crates.io at 3.3.4 with only the
  in-memory engine and HTTP, and without their `allocator` feature, which makes jemalloc the
  global allocator (wrapped in an allocation counter): no jemalloc is linked but the suite's.
  SurrealDB's release profile: fat LTO, one codegen unit, `panic = "abort"`, stripped. Each
  allocator's build is a long single-core link.
- **Load** (`fetch.sh`): 50,000 SurrealQL requests in turn over `POST /sql`: half record
  reads by id, one in five reads of 20 records of a country, one in ten reads by e-mail
  address, and one in five updates of a record (a new version in the store).

`docs/research/servers-surrealdb.md` has the measurements.
