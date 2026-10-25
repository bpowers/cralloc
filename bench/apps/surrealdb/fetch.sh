#!/usr/bin/env bash
# SurrealDB's inputs:
#
# requests.http: what the load generator sends (loadgen --requests), 50,000 whole HTTP requests
# in turn, SurrealQL over POST /sql against the table the driver fills (src/main.rs): half
# record reads by id, one in five reads of 20 records of a country (an index), one in ten a
# read by e-mail address (a unique index), and one in five an update of a record's visit count
# and note, a new version of the record. Records are drawn uniformly from the 750,000. Drawn
# with a fixed seed, then checked.
set -euo pipefail
dest="$1"
here=$(cd "$(dirname "$0")" && pwd)
mkdir -p "$dest"
python3 -I - "$dest" <<'EOF'
import random
import sys

dest = sys.argv[1]
RECORDS = 750_000
COUNTRIES = ("AR AT AU BE BR CA CH CL CN CO CZ DE DK EG ES FI FR GB GR HU ID IE IL IN IT JP KE KR MX MY NG NL NO NZ "
             "PE PH PK PL PT RO SE SG TH TR TW UA US VN ZA ZM").split()
WORDS = "river stone quiet garden north signal paper window summer copper letter harbor".split()
rng = random.Random(20261024)
out = bytearray()
for _ in range(50_000):
    x = rng.random()
    n = rng.randrange(RECORDS)
    if x < 0.5:
        q = f"SELECT * FROM user:{n};"
    elif x < 0.7:
        q = f"SELECT id, name, age FROM user WHERE country = '{rng.choice(COUNTRIES)}' LIMIT 20;"
    elif x < 0.8:
        q = f"SELECT * FROM user WHERE email = 'user{n}@example.com';"
    else:
        note = " ".join(rng.choice(WORDS) for _ in range(rng.randrange(5, 30)))
        q = f"UPDATE user:{n} SET visits += 1, note = '{note}' RETURN NONE;"
    body = q.encode()
    head = (f"POST /sql HTTP/1.1\r\nHost: bench\r\nAccept: application/json\r\nsurreal-ns: bench\r\n"
            f"surreal-db: bench\r\nContent-Length: {len(body)}\r\n\r\n")
    out += head.encode() + body
open(f"{dest}/requests.http", "wb").write(out)
EOF
(cd "$dest" && sha256sum --check --quiet "$here/inputs.sha256")
