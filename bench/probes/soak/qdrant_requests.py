#!/usr/bin/env python3
"""Writes the request files the Qdrant soak sends besides the suite's searches and mixed load
(bench/apps/qdrant), whole HTTP/1.1 requests back to back for `loadgen --requests`:

  qdrant_requests.py OUT_DIR [DIM=384]

- `insert.http`: 400 upserts of 20 new points each, ids 2,000,000 on (the collection's own
  are 0 to 999,999), so that an insert phase grows the collection;
- `delete.http`: deletes of the same 400 batches, so that a delete phase takes it back to the
  points it started with (the driver checks that count when it stops);
- `scratch-insert.http` and `scratch-search.http`: 400 upserts of 20 points into a second
  collection, `scratch`, which the soak creates before and drops after, and queries on it.

No write waits for its indexing (`wait=false`): on the shared cores, upserts that wait queue
behind the optimizers for minutes, and the load generator would count them as errors. The
vectors are drawn around 64 centres from
a fixed seed, so the files are the same on every run.
"""
import json
import os
import random
import sys

BATCHES, BATCH = 400, 20
BASE = 2_000_000


def request(method, path, body):
    data = json.dumps(body, separators=(",", ":")).encode()
    head = (f"{method} {path} HTTP/1.1\r\nHost: bench\r\nContent-Type: application/json\r\n"
            f"Content-Length: {len(data)}\r\n\r\n")
    return head.encode() + data


def main():
    out, dim = sys.argv[1], int(sys.argv[2]) if len(sys.argv) > 2 else 384
    os.makedirs(out, exist_ok=True)
    r = random.Random(20261027)
    centres = [[r.gauss(0, 1) for _ in range(dim)] for _ in range(64)]

    def vector():
        c = r.choice(centres)
        return [round(x + 0.35 * r.gauss(0, 1), 4) for x in c]

    def points(first):
        return [{"id": first + j, "vector": vector(),
                 "payload": {"category": f"c{(first + j) % 100}", "title": f"soak {first + j}"}}
                for j in range(BATCH)]

    files = {name: open(os.path.join(out, name), "wb") for name in
             ("insert.http", "delete.http", "scratch-insert.http", "scratch-search.http")}
    for b in range(BATCHES):
        first = BASE + b * BATCH
        files["insert.http"].write(request("PUT", "/collections/bench/points?wait=false",
                                           {"points": points(first)}))
        files["delete.http"].write(request("POST", "/collections/bench/points/delete?wait=false",
                                           {"points": list(range(first, first + BATCH))}))
        files["scratch-insert.http"].write(request("PUT", "/collections/scratch/points?wait=false",
                                                   {"points": points(b * BATCH)}))
        files["scratch-search.http"].write(request("POST", "/collections/scratch/points/query",
                                                   {"query": vector(), "limit": 10,
                                                    "with_payload": True}))
    for f in files.values():
        f.close()


if __name__ == "__main__":
    main()
