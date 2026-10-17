"""Allocation-heavy Python with threads, subprocesses and a fork: dicts, lists, strings,
bytes, JSON, regular expressions, compression, and objects moving between threads. Prints a
digest that does not depend on scheduling."""

import concurrent.futures
import hashlib
import json
import os
import queue
import re
import subprocess
import sys
import threading
import zlib


def work(seed):
    rng = seed * 2654435761 % 2**32
    out = hashlib.sha256()
    table = {}
    for i in range(30000):
        rng = (rng * 1103515245 + 12345) % 2**31
        key = f"k{rng % 4000}"
        table.setdefault(key, []).append("x" * (rng % 300))
        if len(table[key]) > 20:
            del table[key][:10]
        if i % 500 == 0:
            blob = json.dumps(table, sort_keys=True).encode()
            out.update(zlib.compress(blob, 6))
            out.update(str(len(re.findall(r"k\d+", blob.decode()))).encode())
    big = [bytes([i % 256]) * (i * 1000) for i in range(64)]
    out.update(b"".join(big))
    return out.hexdigest()


def pipeline(n):
    q = queue.Queue(maxsize=64)
    results = []

    def produce():
        for i in range(n):
            q.put({"i": i, "payload": [str(i)] * (i % 50), "blob": os.urandom(0) + b"y" * (i % 4096)})
        q.put(None)

    def consume():
        total = 0
        while (item := q.get()) is not None:
            total += item["i"] + len(item["payload"]) + len(item["blob"])
        results.append(total)

    threads = [threading.Thread(target=produce), threading.Thread(target=consume)]
    for t in threads:
        t.start()
    for t in threads:
        t.join()
    return results[0]


def main():
    with concurrent.futures.ThreadPoolExecutor(max_workers=16) as pool:
        digests = list(pool.map(work, range(32)))
    print("work", hashlib.sha256("".join(digests).encode()).hexdigest())
    print("pipeline", pipeline(50000))
    child = subprocess.run([sys.executable, "-c", "print(sum(len(str(i)) for i in range(10**5)))"],
                           capture_output=True, text=True, check=True)
    print("subprocess", child.stdout.strip())
    pid = os.fork()
    if pid == 0:
        data = [list(range(i % 100)) for i in range(20000)]
        os._exit(0 if sum(map(len, data)) == 990000 else 1)
    _, status = os.waitpid(pid, 0)
    print("fork child", os.waitstatus_to_exitcode(status))


if __name__ == "__main__":
    main()
