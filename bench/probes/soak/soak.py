#!/usr/bin/env python3
"""A soak: one server for hours under a load that swings and goes quiet, sampled every 10 s,
checked as it goes. docs/research/soak.md has the method and the results.

  soak.py --kind qdrant|connect --out DIR --hours H [--seed N] [--ref DIR] [--memstat]
          -- SERVER COMMAND...

The server command is started as given (so wrap it in scripts/heavy and scripts/memlimit) and
must print `BENCH_LISTEN addr`. The schedule (`schedule`) is a list of cycles drawn from the
seed: a few load phases of varying kind, rate, connections and length, each one invocation of
the load generator (so connections open and close between them), then a quiet stretch with
no connection at all, a few of them 10 to 20 minutes. Two runs with the same seed and kind run
the same schedule.

Written to DIR:
- `series.jsonl`, every 10 s: the server's `smaps_rollup`, its number of mappings, minor and
  major faults, its CPU time and the asmalloc background thread's (the thread named
  `asmalloc`), and its cgroup's `memory.current` and `memory.stat` lines;
- `phases.jsonl`: each load phase, the load generator's counts (requests, errors, reconnects,
  unexpected responses) and the peaks of the series over it;
- `quiet.jsonl`: each quiet stretch's last sample, the background thread's CPU over its second
  half, and with --memstat the sampler's last line (bench/alloc/src/memstat.rs: `free`, the
  free bytes on pages mincore calls resident, against `retained`, what the allocator counts);
- `FINDING.json` when a check fails (`check`): the run then sends no more load, leaves the
  server running, and exits 3. At the end the server gets SIGTERM.

With --ref, the checks that compare with a reference run (jemalloc's) read its files as they
grow.
"""
import argparse
import glob
import json
import os
import random
import re
import signal
import subprocess
import sys
import threading
import time
import urllib.request

HERE = os.path.dirname(os.path.abspath(__file__))
BENCH = os.path.abspath(os.path.join(HERE, "..", ".."))
LOADGEN = os.path.join(BENCH, "target", "loadgen", "release", "loadgen")
QDRANT_INPUTS = os.path.join(os.path.dirname(BENCH), "bench", "apps", "target", "inputs", "qdrant")
MIB = 1 << 20
TICK = os.sysconf("SC_CLK_TCK")
SAMPLE_S = 10


# ---------------------------------------------------------------------------------------------
# The schedule.

def quiet_secs(r):
    # One in six is long: the background thread parks, khugepaged gets time, the clock jumps.
    return r.choice([600, 900, 1200]) if r.random() < 1 / 6 else r.choice([60, 120, 180, 300, 480])


def qdrant_cycle(r, k):
    phases = []
    for _ in range(r.randint(1, 3)):
        kind = r.choice(["search", "search", "mixed", "mixed", "closed", "insert"])
        secs = r.choice([60, 120, 180, 300])
        conns = r.choice([2, 4, 8, 16, 32, 64])
        if kind == "closed":
            phases.append({"kind": "closed", "file": "search.http", "conns": r.choice([2, 4, 8]),
                           "secs": secs})
        elif kind == "insert":
            phases.append({"kind": "insert", "file": "insert.http", "rate": r.choice([5, 10, 20]),
                           "conns": r.choice([2, 4, 8]), "secs": secs})
        else:
            # Rates the server keeps up with on the shared cores, beside the reference run: a
            # backlog left at the end of a phase counts as errors.
            phases.append({"kind": kind, "file": f"{kind}.http", "rate": r.choice([50, 100, 150, 200]),
                           "conns": conns, "secs": secs})
    # What was inserted goes again in the same cycle, so the collection keeps its size.
    if any(p["kind"] == "insert" for p in phases):
        phases.append({"kind": "delete", "file": "delete.http", "rate": 20, "conns": 4, "secs": 30})
    # Every fourth cycle a second collection is made, filled, searched and dropped.
    if k % 4 == 3:
        phases += [
            {"kind": "scratch-create"},
            {"kind": "scratch-insert", "file": "scratch-insert.http", "rate": r.choice([20, 40]),
             "conns": 4, "secs": r.choice([60, 120])},
            {"kind": "scratch-search", "file": "scratch-search.http", "rate": 100, "conns": 8,
             "secs": 60},
            {"kind": "scratch-drop"},
        ]
    return phases


def connect_cycle(r, k):
    phases = []
    for _ in range(r.randint(1, 3)):
        # At most 180 s: the store closes an HTTP/1.1 connection idle for 300 s.
        secs = r.choice([60, 120, 180])
        if r.random() < 0.5:
            # The store's own mix over HTTP/1.1: ingest 50 records, tail 100.
            phases.append({"kind": "store", "file": "store.http", "rate": r.choice([1000, 2000, 4000, 6000]),
                           "conns": r.choice([4, 16, 64, 128]), "secs": secs})
        else:
            # Ingest over HTTP/2, batches of 5, 50 or 500 records.
            records = r.choice([5, 50, 500])
            phases.append({"kind": "ingest-h2", "body": f"log{records}.bin", "records": records,
                           "rate": min(8000, r.choice([200, 1000, 3000]) * 50 // records),
                           "conns": r.choice([1, 2, 8, 16]), "streams": r.choice([1, 8, 32]),
                           "secs": secs})
    return phases


def schedule(kind, seed, hours):
    r = random.Random(seed)
    cycles, total, k = [], 0, 0
    while total < hours * 3600:
        phases = (qdrant_cycle if kind == "qdrant" else connect_cycle)(r, k)
        quiet = quiet_secs(r)
        cycles.append({"phases": phases, "quiet": quiet})
        total += sum(p.get("secs", 5) for p in phases) + quiet
        k += 1
    return cycles


# ---------------------------------------------------------------------------------------------
# Reading the server.

def read(path):
    try:
        with open(path) as f:
            return f.read()
    except OSError:
        return None


def children(pid):
    out = []
    for stat in glob.glob("/proc/[0-9]*/stat"):
        text = read(stat)
        if text and int(text[text.rindex(")") + 2:].split()[1]) == pid:
            out.append(int(stat.split("/")[2]))
    return out


def descendants(pid):
    found, todo = [], [pid]
    while todo:
        for c in children(todo.pop()):
            found.append(c)
            todo.append(c)
    return found


def cmdline(pid):
    text = read(f"/proc/{pid}/cmdline") or ""
    return text.split("\0")


def sample(pid):
    s = {"t": time.time()}
    rollup = read(f"/proc/{pid}/smaps_rollup")
    if rollup is None:
        return None
    s["rollup"] = {m.group(1): int(m.group(2)) for m in re.finditer(r"^(\w+):\s+(\d+) kB", rollup, re.M)}
    maps = read(f"/proc/{pid}/maps")
    s["maps"] = maps.count("\n") if maps else 0
    stat = read(f"/proc/{pid}/stat")
    f = stat[stat.rindex(")") + 2:].split()
    s["minflt"], s["majflt"] = int(f[7]), int(f[9])
    s["cpu_ms"] = (int(f[11]) + int(f[12])) * 1000 // TICK
    bg = 0
    for task in glob.glob(f"/proc/{pid}/task/*"):
        if (read(task + "/comm") or "").strip() == "asmalloc":
            t = read(task + "/stat")
            if t:
                g = t[t.rindex(")") + 2:].split()
                bg += (int(g[11]) + int(g[12])) * 1000 // TICK
    s["bg_cpu_ms"] = bg
    cg = (read(f"/proc/{pid}/cgroup") or "").strip().split("::")[-1]
    base = "/sys/fs/cgroup" + cg
    cur = read(base + "/memory.current")
    if cur:
        s["cg_current"] = int(cur)
        st = read(base + "/memory.stat") or ""
        s["cg"] = {k: int(v) for k, v in (l.split() for l in st.splitlines())
                   if k in ("anon", "file", "anon_thp", "shmem", "thp_fault_alloc", "thp_collapse_alloc")}
    return s


def memstat_last(pattern, pid):
    path = f"{pattern}.{pid}"
    try:
        with open(path, "rb") as f:
            f.seek(0, 2)
            size = f.tell()
            f.seek(max(0, size - 65536))
            lines = f.read().decode(errors="replace").splitlines()
    except OSError:
        return None
    for line in reversed(lines):
        if line.startswith("sample "):
            kv = dict(re.findall(r"(\w+)=(\S+)", line))
            return {k: int(v) for k, v in kv.items() if v.isdigit()}
    return None


# ---------------------------------------------------------------------------------------------
# The checks.

def median(xs):
    xs = sorted(xs)
    return xs[len(xs) // 2] if xs else 0


def jsonl(path):
    out = []
    text = read(path) or ""
    for line in text.splitlines():
        try:
            out.append(json.loads(line))
        except ValueError:
            pass
    return out


def anon(q):
    return q["last"]["rollup"].get("Anonymous", 0) * 1024


def check(out, ref, memstat):
    """The findings the lead asked to be told of at once, or None."""
    quiet = jsonl(os.path.join(out, "quiet.jsonl"))
    phases = jsonl(os.path.join(out, "phases.jsonl"))
    rquiet = jsonl(os.path.join(ref, "quiet.jsonl")) if ref else []
    rphases = jsonl(os.path.join(ref, "phases.jsonl")) if ref else []
    # Error responses that the reference's same phase does not have.
    for i, p in enumerate(phases):
        bad = p.get("errors", 0) + p.get("reconnects", 0) + p.get("unexpected", 0)
        if bad and (not ref or (i < len(rphases) and rphases[i].get("errors", 0)
                                + rphases[i].get("reconnects", 0) + rphases[i].get("unexpected", 0) == 0)):
            return {"what": "errors", "phase": i, "ours": p, "ref": rphases[i] if i < len(rphases) else None}
    if len(quiet) >= 10:
        first, last = quiet[2:7], quiet[-5:]
        rise = median(anon(q) for q in last) - median(anon(q) for q in first)
        n = min(len(quiet), len(rquiet))
        if ref and n >= 10:
            rrise = median(anon(q) for q in rquiet[n - 5:n]) - median(anon(q) for q in rquiet[2:7])
            margin_then = median(anon(quiet[i]) - anon(rquiet[i]) for i in range(2, 7))
            margin_now = median(anon(quiet[i]) - anon(rquiet[i]) for i in range(n - 5, n))
            if rise > 64 * MIB and rise > 3 * max(rrise, 0):
                return {"what": "quiet resident climbs", "rise": rise, "ref_rise": rrise}
            if margin_now - margin_then > 64 * MIB:
                return {"what": "margin over jemalloc grows", "then": margin_then, "now": margin_now}
        maps_first = median(q["last"]["maps"] for q in first)
        maps_last = median(q["last"]["maps"] for q in last)
        if maps_last > 1.5 * maps_first + 200:
            return {"what": "mappings grow", "first": maps_first, "last": maps_last}
    if quiet and quiet[-1]["last"]["maps"] > 30000:
        return {"what": "mappings", "maps": quiet[-1]["last"]["maps"]}
    # The background thread awake through the second half of two quiet stretches in a row.
    busy = [q for q in quiet if q.get("bg_share") is not None]
    if len(busy) >= 2 and all(q["bg_share"] > 0.02 for q in busy[-2:]):
        return {"what": "background thread busy while quiet", "shares": [q["bg_share"] for q in busy[-2:]]}
    if memstat and quiet and quiet[-1].get("memstat"):
        m = quiet[-1]["memstat"]
        gap = m.get("free", 0) - m.get("retained", 0)
        if abs(gap) > 48 * MIB:
            return {"what": "count and kernel apart", "gap": gap, "memstat": m}
    # Huge pages draining: the peak AnonHugePages under load, late against early.
    peaks = [p["peak_anonhuge_kib"] for p in phases if p.get("kind") in ("search", "mixed", "closed", "store")]
    if len(peaks) >= 20:
        early, late = median(peaks[2:10]), median(peaks[-8:])
        if early > 32 * 1024 and late < early / 4:
            return {"what": "huge pages drained", "early_kib": early, "late_kib": late}
    return None


# ---------------------------------------------------------------------------------------------
# Running it.

class Sampler(threading.Thread):
    def __init__(self, pid, out):
        super().__init__(daemon=True)
        self.pid, self.out = pid, open(os.path.join(out, "series.jsonl"), "a")
        self.phase = "start"
        self.last = None
        self.peak = {}
        self.lock = threading.Lock()

    def take(self):
        s = sample(self.pid)
        if s is None:
            return None
        s["phase"] = self.phase
        with self.lock:
            self.last = s
            r = s["rollup"]
            for k, v in (("rss_kib", r.get("Rss", 0)), ("anonhuge_kib", r.get("AnonHugePages", 0)),
                         ("anon_kib", r.get("Anonymous", 0)), ("maps", s["maps"])):
                self.peak[k] = max(self.peak.get(k, 0), v)
        self.out.write(json.dumps(s) + "\n")
        self.out.flush()
        return s

    def reset_peaks(self):
        with self.lock:
            self.peak = {}

    def peaks(self):
        with self.lock:
            return dict(self.peak)

    def run(self):
        while True:
            if self.take() is None:
                return
            time.sleep(SAMPLE_S)


def http(addr, method, path, body=None):
    req = urllib.request.Request(f"http://{addr}{path}", method=method,
                                 data=json.dumps(body).encode() if body is not None else None,
                                 headers={"Content-Type": "application/json"})
    try:
        with urllib.request.urlopen(req, timeout=600) as resp:
            return resp.status
    except Exception as e:  # noqa: BLE001 - recorded, not raised
        return f"{e}"


def loadgen(addr, phase, inputs):
    args = [LOADGEN, "--addr", addr, "--conns", str(phase["conns"]), "--threads", str(min(2, phase["conns"])),
            "--drain-s", "120", "--label", phase["kind"]]
    secs = phase["secs"]
    # Every connection sends one request first: a server closes a connection that has sent
    # nothing for a few seconds (Qdrant's actix after 5 s), which at a low rate would leave
    # connections the load never reached to be closed under it.
    warm = "_warm:closed:0.2,"
    if phase["kind"] == "closed":
        args += ["--plan", f"{warm}p:closed:{secs}"]
    else:
        args += ["--plan", f"{warm}p:{phase['rate']}:{secs}"]
    if phase["kind"] == "ingest-h2":
        args += ["--proto", "h2", "--streams", str(phase["streams"]),
                 "--path", "/bench.v1.LogIngestService/Ingest", "--content-type", "application/proto",
                 "--header", "Connect-Protocol-Version: 1", "--body", os.path.join(inputs, phase["body"])]
    else:
        args += ["--requests", os.path.join(inputs, phase["file"])]
    run = subprocess.run(args, capture_output=True, text=True)
    result = {}
    for line in run.stdout.splitlines():
        if line.startswith("RESULT "):
            result = json.loads(line[7:])
    seg = result.get("groups", {}).get("p", {})
    return {"exit": run.returncode, "rate": seg.get("rate", 0), "errors": seg.get("errors", 0),
            "missed": seg.get("missed", 0), "reconnects": result.get("reconnects", 0),
            "unexpected": result.get("unexpected_responses", 0),
            "stderr": run.stderr[-400:] if run.returncode else ""}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--kind", required=True, choices=["qdrant", "connect"])
    ap.add_argument("--out", required=True)
    ap.add_argument("--hours", type=float, required=True)
    ap.add_argument("--seed", type=int, default=1)
    ap.add_argument("--ref")
    ap.add_argument("--reference", action="store_true",
                    help="this run is the reference (jemalloc's): it stops only if its server dies")
    ap.add_argument("--memstat", help="the ASMALLOC_MEMSTAT path the server writes to (.<pid> added)")
    ap.add_argument("--inputs", required=True)
    ap.add_argument("--scale", type=float, default=1, help="divide every length by this (a trial run)")
    ap.add_argument("cmd", nargs=argparse.REMAINDER)
    a = ap.parse_args()
    cmd = a.cmd[1:] if a.cmd and a.cmd[0] == "--" else a.cmd
    os.makedirs(a.out, exist_ok=True)
    plan = schedule(a.kind, a.seed, a.hours * a.scale)
    for cycle in plan:
        cycle["quiet"] = max(2, cycle["quiet"] / a.scale)
        for p in cycle["phases"]:
            if "secs" in p:
                p["secs"] = max(2, p["secs"] / a.scale)
    with open(os.path.join(a.out, "schedule.json"), "w") as f:
        json.dump(plan, f)
    log = open(os.path.join(a.out, "server.log"), "w")
    server = subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=log, text=True, start_new_session=True)
    addr = None
    for line in server.stdout:
        if line.startswith("BENCH_LISTEN "):
            addr = line.split()[1]
            break
    if addr is None:
        sys.exit(f"soak: the server never listened (see {a.out}/server.log)")
    # The rest of its output goes to the log, so the pipe never fills.
    threading.Thread(target=lambda: [log.write(l) for l in server.stdout], daemon=True).start()

    def is_server(p):
        c = cmdline(p)
        exe = os.path.basename(c[0]) if c else ""
        if a.kind == "qdrant":
            return len(c) > 1 and c[1].startswith("--config-path")
        return exe == "rpc-server"

    pid = next((p for p in descendants(server.pid) if is_server(p)), None)
    if pid is None:
        sys.exit("soak: no server process under the command")
    events = open(os.path.join(a.out, "events.jsonl"), "a")

    def event(**kv):
        kv["t"] = time.time()
        events.write(json.dumps(kv) + "\n")
        events.flush()

    event(what="start", addr=addr, pid=pid, cmd=cmd)
    sampler = Sampler(pid, a.out)
    sampler.start()
    phases_out = open(os.path.join(a.out, "phases.jsonl"), "a")
    quiet_out = open(os.path.join(a.out, "quiet.jsonl"), "a")

    def alive():
        return os.path.exists(f"/proc/{pid}") and server.poll() is None

    def finding(f):
        f["t"] = time.time()
        with open(os.path.join(a.out, "FINDING.json"), "w") as out:
            json.dump(f, out, indent=1)
        event(what="finding", finding=f)
        print(f"FINDING {json.dumps(f)}", flush=True)
        sys.exit(3)

    for k, cycle in enumerate(plan):
        for phase in cycle["phases"]:
            if not alive():
                finding({"what": "server gone", "exit": server.poll(), "cycle": k})
            sampler.phase = f"{k}:{phase['kind']}"
            sampler.reset_peaks()
            t0 = time.time()
            if phase["kind"] == "scratch-create":
                status = http(addr, "PUT", "/collections/scratch",
                              {"vectors": {"size": 384, "distance": "Cosine", "on_disk": False},
                               "hnsw_config": {"m": 16, "ef_construct": 100}})
                result = {"status": status}
            elif phase["kind"] == "scratch-drop":
                result = {"status": http(addr, "DELETE", "/collections/scratch")}
            else:
                result = loadgen(addr, phase, a.inputs)
            if "status" in result and result["status"] != 200:
                result["errors"] = 1
            sampler.take()
            peaks = sampler.peaks()
            rec = {"cycle": k, "kind": phase["kind"], "phase": phase, "secs": time.time() - t0, **result,
                   **{f"peak_{key}": v for key, v in peaks.items()}}
            phases_out.write(json.dumps(rec) + "\n")
            phases_out.flush()
            f = None if a.reference else check(a.out, a.ref, a.memstat)
            if f:
                finding(f)
        if not alive():
            finding({"what": "server gone", "exit": server.poll(), "cycle": k})
        sampler.phase = f"{k}:quiet"
        q = cycle["quiet"]
        t0 = time.time()
        half = min(q / 2, max(30, q / 2))
        time.sleep(half)
        mid = sampler.take()
        time.sleep(max(0, q - half))
        end = sampler.take()
        if end is None:
            finding({"what": "server gone", "exit": server.poll(), "cycle": k})
        rec = {"cycle": k, "secs": time.time() - t0, "last": end}
        if mid and end["t"] > mid["t"]:
            rec["bg_share"] = (end["bg_cpu_ms"] - mid["bg_cpu_ms"]) / 1000 / (end["t"] - mid["t"])
            rec["cpu_share"] = (end["cpu_ms"] - mid["cpu_ms"]) / 1000 / (end["t"] - mid["t"])
        if a.memstat:
            rec["memstat"] = memstat_last(a.memstat, pid)
        quiet_out.write(json.dumps(rec) + "\n")
        quiet_out.flush()
        f = None if a.reference else check(a.out, a.ref, a.memstat)
        if f:
            finding(f)
    event(what="end")
    if a.kind == "qdrant":
        # Every point the soak inserted goes before the driver counts the collection, and the
        # scratch collection with them if a finding stopped a cycle short.
        http(addr, "DELETE", "/collections/scratch")
        loadgen(addr, {"kind": "delete", "file": "delete.http", "rate": 50, "conns": 4, "secs": 20},
                a.inputs)
        time.sleep(60)
    # SIGTERM to the server's driver (the process that printed BENCH_LISTEN for Qdrant, the
    # server itself for connect), which stops it cleanly; the wrappers then exit.
    target = pid if a.kind == "connect" else next(
        (p for p in descendants(server.pid) if "serve" in cmdline(p) and p != pid), pid)
    os.kill(target, signal.SIGTERM)
    try:
        server.wait(timeout=180)
    except subprocess.TimeoutExpired:
        event(what="server did not stop")
    event(what="stopped", exit=server.returncode)
    print(f"soak: done, server exit {server.returncode}", flush=True)


if __name__ == "__main__":
    main()
