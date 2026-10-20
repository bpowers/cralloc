#!/usr/bin/env bash
# The command list the load generator sends to mini-redis (loadgen --proto resp --requests):
# 200,000 commands over 100,000 keys, alternately SET and GET, keys and value sizes (32 to
# 512 bytes) from a fixed hash, so the heap holds about 30 MB of values in steady state and
# the server allocates and frees values of many sizes. Generated, then checked.
set -euo pipefail
dest="$1"
python3 -I - "$dest/commands.resp" <<'EOF'
import hashlib
import sys

def h(i, salt):
    return int.from_bytes(hashlib.blake2b(f"{salt}{i}".encode(), digest_size=8).digest(), "little")

def cmd(*parts):
    out = [f"*{len(parts)}\r\n".encode()]
    for p in parts:
        out.append(f"${len(p)}\r\n".encode() + p + b"\r\n")
    return b"".join(out)

with open(sys.argv[1], "wb") as f:
    for i in range(200_000):
        key = f"key:{h(i, 'k') % 100_000:06d}".encode()
        if i % 2 == 0:
            n = 32 + h(i, "n") % 481
            f.write(cmd(b"SET", key, (b"%08d" % i) * (n // 8) + b"x" * (n % 8)))
        else:
            f.write(cmd(b"GET", key))
EOF
echo "e5b203d76bfdcc1be3766c60c674a1aad2e145688eaa1636bfea35a27a3b80a0  $dest/commands.resp" | sha256sum --check --quiet
