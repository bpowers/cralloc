#!/usr/bin/env bash
# The Router's inputs:
#
# bin/protoc: the Router's telemetry compiles protobuf with protoc at build time, which the
# machine may not have: protobuf's own release of it, checked against a digest, into
# DEST/bin, which build-apps puts first on PATH for this program's builds (as Sōzu's).
#
# requests.http: what the load generator sends (loadgen --requests), 20,000 whole HTTP
# requests in turn, GraphQL POSTs to the Router's endpoint against the supergraph in
# ../supergraph.graphql: three in four `TopProducts` (a page of 10, 20 or 30 products from
# one of 64 offsets, each with its inventory, its reviews and their authors: four subgraphs,
# three fetches deep), one in eight `Me` (the user, their reviews and those reviews' products)
# and one in eight `Recommended` (twenty products from the accounts subgraph, with fields from
# the other three). Drawn with a fixed seed, then checked.
set -euo pipefail
dest="$1"
here=$(cd "$(dirname "$0")" && pwd)
"$here/../protoc.sh" "$dest"

python3 -I - "$dest" <<'EOF'
import json
import random
import sys

dest = sys.argv[1]
TOP = """query TopProducts($first: Int!, $offset: Int!) {
  topProducts(first: $first, offset: $offset) {
    upc name price description inStock shippingEstimate
    reviews { id body rating author { id username name } }
  }
}"""
ME = """query Me {
  me {
    id name username
    reviews { id body rating product { upc name price inStock } }
  }
}"""
RECOMMENDED = """query Recommended {
  recommendedProducts {
    upc name price weight shippingEstimate
    reviews { rating author { username } }
  }
}"""
rng = random.Random(20261024)
out = bytearray()
for _ in range(20_000):
    x = rng.random()
    if x < 0.75:
        op = {
            "operationName": "TopProducts",
            "query": TOP,
            "variables": {"first": rng.choice([10, 20, 30]), "offset": 150 * rng.randrange(64)},
        }
    elif x < 0.875:
        op = {"operationName": "Me", "query": ME}
    else:
        op = {"operationName": "Recommended", "query": RECOMMENDED}
    body = json.dumps(op, separators=(",", ":")).encode()
    head = f"POST / HTTP/1.1\r\nHost: bench\r\nContent-Type: application/json\r\nContent-Length: {len(body)}\r\n\r\n"
    out += head.encode() + body
open(f"{dest}/requests.http", "wb").write(out)
EOF
(cd "$dest" && sha256sum --check --quiet "$here/inputs.sha256")
