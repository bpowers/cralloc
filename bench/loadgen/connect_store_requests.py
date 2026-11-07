#!/usr/bin/env python3
"""Writes the requests loadgen sends to `rpc-server store` (bench/rpc/src/store.rs), whole
HTTP/1.1 requests back to back for `loadgen --requests`:

  connect_store_requests.py PAIRS TAIL > store.http

PAIRS times, an ingest of a 50-record batch (`LogIngestService/Ingest`, the Connect unary
protocol, binary protobuf) and a tail of the TAIL most recent records
(`BenchService/ServerStream`, the Connect streaming protocol: the request one enveloped
message, the response a stream of them). The records are `log_request`'s, numbered on from
10,000,000 so that every batch differs from the next and from the ones the server starts with.
"""
import os
import sys

# python3 -I leaves the script's directory off the path.
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from connect_log_body import ld, record, vint  # noqa: E402


def request(path, ctype, body):
    head = (f"POST {path} HTTP/1.1\r\nHost: bench\r\nContent-Type: {ctype}\r\n"
            f"Connect-Protocol-Version: 1\r\nContent-Length: {len(body)}\r\n\r\n")
    return head.encode() + body


def main():
    pairs, tail = int(sys.argv[1]), int(sys.argv[2])
    tail_body = vint(1, tail)
    tail_req = request("/bench.v1.BenchService/ServerStream", "application/connect+proto",
                       b"\0" + len(tail_body).to_bytes(4, "big") + tail_body)
    out = sys.stdout.buffer
    for p in range(pairs):
        first = 10_000_000 + p * 50
        batch = b"".join(ld(1, record(i)) for i in range(first, first + 50))
        out.write(request("/bench.v1.LogIngestService/Ingest", "application/proto", batch))
        out.write(tail_req)


if __name__ == "__main__":
    main()
