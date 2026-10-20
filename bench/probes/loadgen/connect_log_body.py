#!/usr/bin/env python3
"""Writes the request body of connect-rust's log-ingest benchmark (bench/rpc, `log_request`)
as binary protobuf, for loadgen to send to `rpc-server log` with the Connect protocol over
HTTP/1.1:

  connect_log_body.py [RECORDS=50] > log50.bin
  loadgen --path /bench.v1.LogIngestService/Ingest --content-type application/proto \
          --header 'Connect-Protocol-Version: 1' --body log50.bin ...

The records are built field for field as bench/rpc/src/lib.rs builds them; the map entries
are written in key order, which a decoder does not care about.
"""
import sys


def varint(n):
    n &= (1 << 64) - 1
    out = bytearray()
    while True:
        b = n & 0x7F
        n >>= 7
        if n:
            out.append(b | 0x80)
        else:
            out.append(b)
            return bytes(out)


def key(field, wire):
    return varint(field << 3 | wire)


def vint(field, n):
    return key(field, 0) + varint(n)


def ld(field, data):
    if isinstance(data, str):
        data = data.encode()
    return key(field, 2) + varint(len(data)) + data


def record(i):
    labels = b"".join(ld(6, ld(1, f"label-key-{j}") + ld(2, f"label-value-{i}-{j}")) for j in range(6))
    message = (f"Processing request from client {i}: GET /api/v1/users?page={i % 100}&limit=50 "
               f"completed in {42 + i % 200}ms with status 200 OK")
    source = ld(1, f"src/handlers/user_{i % 10}.rs") + vint(2, 42 + i % 500) + ld(3, f"handle_get_users_{i % 5}")
    return (vint(1, 1_700_000_000_000_000_000 + i) + ld(2, "api-gateway") + ld(3, f"instance-{i:04x}")
            + vint(4, 2) + ld(5, message) + labels + ld(7, f"{0xDEADBEEF00000000 + i:032x}")
            + ld(8, f"{0xCAFE0000 + i:016x}") + ld(9, source))


def main():
    n = int(sys.argv[1]) if len(sys.argv) > 1 else 50
    sys.stdout.buffer.write(b"".join(ld(1, record(i)) for i in range(n)))


if __name__ == "__main__":
    main()
