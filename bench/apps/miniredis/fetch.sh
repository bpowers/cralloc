#!/usr/bin/env bash
# The command lists the load generator sends to mini-redis (loadgen --proto resp --requests).
#
# commands.resp: 200,000 commands over 100,000 keys, alternately SET and GET, keys and value
# sizes (32 to 512 bytes) from a fixed hash, so the heap holds about 30 MB of values in steady
# state and the server allocates and frees values of many sizes.
#
# churn-<keys>.resp, for the large-heap jobs whose server is preloaded with <keys> keys
# (`miniredis serve preload=<keys>`): 200,000 commands over that keyspace, half GET and half
# SET; a SET overwrites a random key with a value of 64 to 4,096 bytes (log-uniform), and one
# SET in ten sets an expiry of 10 to 60 seconds, so the heap churns through mixed sizes and
# loses and regains keys while it serves.
#
# Generated, then checked.
set -euo pipefail
dest="$1"
here=$(cd "$(dirname "$0")" && pwd)
python3 -I - "$dest" <<'EOF'
import hashlib
import sys
from pathlib import Path

dest = Path(sys.argv[1])


def h(i, salt):
    return int.from_bytes(hashlib.blake2b(f"{salt}{i}".encode(), digest_size=8).digest(), "little")


def cmd(*parts):
    out = [f"*{len(parts)}\r\n".encode()]
    for p in parts:
        out.append(f"${len(p)}\r\n".encode() + p + b"\r\n")
    return b"".join(out)


with open(dest / "commands.resp", "wb") as f:
    for i in range(200_000):
        key = f"key:{h(i, 'k') % 100_000:06d}".encode()
        if i % 2 == 0:
            n = 32 + h(i, "n") % 481
            f.write(cmd(b"SET", key, (b"%08d" % i) * (n // 8) + b"x" * (n % 8)))
        else:
            f.write(cmd(b"GET", key))

for keys in (450_000, 1_800_000, 3_600_000):
    with open(dest / f"churn-{keys}.resp", "wb") as f:
        for i in range(200_000):
            key = f"key:{h(i, 'c') % keys:08d}".encode()
            if i % 2:
                f.write(cmd(b"GET", key))
                continue
            u = (h(i, "s") >> 11) / float(1 << 53)
            n = int(64 * 2 ** (6 * u))
            value = (b"%08d" % i) * (n // 8) + b"x" * (n % 8)
            if h(i, "e") % 10 == 0:
                f.write(cmd(b"SET", key, value, b"EX", str(10 + h(i, "t") % 51).encode()))
            else:
                f.write(cmd(b"SET", key, value))
EOF
(cd "$dest" && sha256sum --check --quiet "$here/inputs.sha256")
