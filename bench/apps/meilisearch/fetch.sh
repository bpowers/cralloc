#!/usr/bin/env bash
# Meilisearch's inputs:
#
# hn-1.ndjson ... hn-5.ndjson: Hacker News items, 100,000 to a file, the dataset of Meilisearch's
# own `hackernews` benchmark workload (its workloads/hackernews.json at v1.55.0), from where
# that workload fetches them, checked against its digests. The server indexes the first four.
#
# requests.http: what the load generator sends (loadgen --requests), 200,000 whole HTTP
# requests in turn. Searches (POST /indexes/hn/search, limit 20) for one to three words drawn
# from the titles of the indexed items, the commoner words more often; and one request in 200
# a batch of 50 indexed items updated (their score raised and a word added to their title),
# which Meilisearch answers at once and indexes in the background. Generated from the items
# with a fixed seed, then checked.
set -euo pipefail
dest="$1"
here=$(cd "$(dirname "$0")" && pwd)
mkdir -p "$dest"
base=https://milli-benchmarks.fra1.digitaloceanspaces.com/bench/datasets/hackernews
for i in 1 2 3 4 5; do
  f="$dest/hn-$i.ndjson"
  [ -s "$f" ] || curl -sfL -o "$f" "$base/hackernews-${i}00_000.ndjson"
done
python3 -I - "$dest" <<'EOF'
import collections
import json
import random
import re
import sys

dest = sys.argv[1]
items = []
for i in range(1, 5):
    with open(f"{dest}/hn-{i}.ndjson") as f:
        items.extend(json.loads(l) for l in f)
words = collections.Counter()
for it in items:
    for w in re.findall(r"[a-z]{3,12}", (it.get("title") or "").lower()):
        words[w] += 1
vocab = [w for w, n in words.most_common(20000) if n >= 3]
weights = [words[w] for w in vocab]
rng = random.Random(20261021)


def request(path, ctype, body):
    head = f"POST {path} HTTP/1.1\r\nHost: bench\r\nContent-Type: {ctype}\r\nContent-Length: {len(body)}\r\n\r\n"
    return head.encode() + body


out = bytearray()
for n in range(200_000):
    if n % 200 == 199:
        batch = []
        for it in rng.sample(items, 50):
            it = dict(it)
            it["score"] = (it.get("score") or 0) + 1
            it["title"] = ((it.get("title") or "") + " " + rng.choice(vocab)).strip()
            batch.append(json.dumps(it, separators=(",", ":")))
        out += request("/indexes/hn/documents", "application/x-ndjson", ("\n".join(batch) + "\n").encode())
    else:
        q = " ".join(rng.choices(vocab, weights, k=rng.randint(1, 3)))
        out += request("/indexes/hn/search", "application/json", json.dumps({"q": q, "limit": 20}).encode())
open(f"{dest}/requests.http", "wb").write(out)
EOF
(cd "$dest" && sha256sum --check --quiet "$here/inputs.sha256")
