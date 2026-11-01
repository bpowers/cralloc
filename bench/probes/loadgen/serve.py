#!/usr/bin/env python3
"""Drives the order services (axum, actix) and connect-rust's log-ingest server with loadgen
under each allocator, interleaved, and records latency, the server's counters and its memory.

  serve.py run SERVER --plan sat|day [--rates R30,R70] [--rounds N] [--allocs a,b] --out DIR
  serve.py report DIR [DIR...]

SERVER is axum, actix (each allocator preloaded, as the screening ran them) or rpc
(`rpc-server log` built with each allocator as its global allocator, as the suite runs it).
`sat` is a closed-loop run to find the saturated rate; `day` is a server's day: saturate,
steady load at the two rates, load that comes and goes, then the connections close and the
server is watched while idle. Each allocator runs in a fresh server process; the order of
the allocators is shuffled every round.

This is a measurement: run it under scripts/measure --both-l3. It caps the server and the
generator with scripts/memlimit itself. Results go to DIR/runs.jsonl, with each run's raw
files in DIR/SERVER/.
"""
import argparse
import json
import os
import random
import signal
import socket
import statistics
import struct
import subprocess
import sys
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[2]
TARGET = Path(os.environ.get("SERVE_TARGET", REPO / "target" / "server-tails"))
SCREEN = Path(os.environ.get("SCREEN_ROOT", REPO / "target" / "rust-screen"))


def main_checkout():
    common = subprocess.run(["git", "-C", str(REPO), "rev-parse", "--path-format=absolute", "--git-common-dir"],
                            capture_output=True, text=True, check=True).stdout.strip()
    return Path(common).parent


BASE = Path(os.environ.get("BENCH_BASELINES", main_checkout() / "bench" / "baselines" / "out"))
LOADGEN = TARGET / "loadgen" / "release" / "loadgen"
MEMLIMIT = REPO / "scripts" / "memlimit"
SERVER_CPUS, CLIENT_CPUS, AUX_CPU = "0-3", "8-11", "15"
EVENTS = "cycles:u,cycles:k,instructions:u,task-clock,page-faults,context-switches,cpu-migrations"

ALLOCS = {
    "glibc": {"preload": "", "build": "glibc", "env": {}},
    "jemalloc": {"preload": str(BASE / "jemalloc" / "libjemalloc.so"), "build": "je", "env": {}},
    "mimalloc": {"preload": str(BASE / "mimalloc" / "libmimalloc.so"), "build": "mi", "env": {}},
    # tcmalloc's per-CPU mode needs glibc's rseq registration off (bench/allocators.toml).
    "tcmalloc": {"preload": str(BASE / "tcmalloc" / "libtcmalloc.so"), "build": "tc",
                 "env": {"GLIBC_TUNABLES": "glibc.pthread.rseq=0"}},
    "cralloc": {"preload": str(REPO / "target" / "release" / "libcralloc_preload.so"), "build": "cralloc",
                 "env": {}},
    # cralloc with its opt-in background thread, which runs the clocked maintenance once an
    # interval whether or not the program allocates. Preload only.
    "cralloc-bg": {"preload": str(REPO / "target" / "release" / "libcralloc_preload.so"), "build": None,
                    "env": {"CRALLOC_BACKGROUND": "1"}},
}
DEFAULT_ALLOCS = "glibc,jemalloc,mimalloc,tcmalloc,cralloc"
# More configurations from the environment, as JSON: {"name": {"bin": PATH, "env": {...},
# "preload": PATH}}. "bin" replaces the server binary (a build with an allocator linked in).
ALLOCS.update({k: {"preload": "", "build": None, "env": {}, **v}
               for k, v in json.loads(os.environ.get("SERVE_EXTRA_ALLOCS", "{}")).items()})

ORDER = str(REPO / "bench" / "probes" / "rust-screen" / "servers" / "order.json")
SERVERS = {
    "axum": {"bin": str(SCREEN / "bin" / "servers" / "axum-orders"), "args": ["28480"], "port": 28480,
             "path": "/orders", "ctype": "application/json", "body": ORDER, "headers": []},
    "actix": {"bin": str(SCREEN / "bin" / "servers" / "actix-orders"), "args": ["28481"], "port": 28481,
              "path": "/orders", "ctype": "application/json", "body": ORDER, "headers": []},
    "rpc": {"native": True, "args": ["log"], "env": {"BENCH_CPUS": SERVER_CPUS},
            "path": "/bench.v1.LogIngestService/Ingest", "ctype": "application/proto",
            "body": str(TARGET / "log50.bin"), "headers": ["Connect-Protocol-Version: 1"]},
}

# A server's day. The silences are long enough for several of the allocators' clocked steps
# (cralloc's interval is about a second); the final `close` watches the idle server.
CYCLES, ON_S, OFF_S, IDLE_S = 4, 2, 5, 20


def day_plan(r30, r70):
    segs = [("_fill", "closed", 3), ("sat", "closed", 4), ("_w30", r30, 1), ("s30", r30, 10),
            ("_w70", r70, 1), ("s70", r70, 10)]
    for _ in range(CYCLES):
        segs += [("off", "off", OFF_S), ("on", r70, ON_S)]
    segs.append(("idle", "close", IDLE_S))
    return segs


def sat_plan():
    return [("_warm", "closed", 2), ("sat", "closed", 5)]


def windows(segs):
    """perf windows as (label, first segment, segment after the last), by segment index."""
    w = []
    for i, (name, _, _) in enumerate(segs):
        if name in ("sat", "s30", "s70"):
            w.append((name, i, i + 1))
    ons = [i for i, s in enumerate(segs) if s[0] in ("on", "off")]
    if ons:
        w.append(("onoff", ons[0], ons[-1] + 1))
    idle = [i for i, s in enumerate(segs) if s[0] == "idle"]
    if idle:
        w.append(("idle", idle[0], idle[0] + 1))
    return w


def descendants(pid):
    kids, todo = [], [pid]
    while todo:
        p = todo.pop()
        try:
            for t in Path(f"/proc/{p}/task").iterdir():
                c = (t / "children").read_text().split()
                kids += [int(x) for x in c]
                todo += [int(x) for x in c]
        except OSError:
            pass
    return kids


def find_server(root, binary):
    want = os.path.basename(binary)[:15]
    for p in descendants(root):
        try:
            if Path(f"/proc/{p}/comm").read_text().strip() == want:
                return p
        except OSError:
            pass
    return None


def wait_port(port, timeout=30.0):
    end = time.monotonic() + timeout
    while time.monotonic() < end:
        try:
            with socket.create_connection(("127.0.0.1", port), timeout=0.2):
                return True
        except OSError:
            time.sleep(0.02)
    return False


def cpu_times():
    out = {}
    for line in Path("/proc/stat").read_text().splitlines():
        f = line.split()
        if f[0].startswith("cpu") and f[0] != "cpu":
            v = list(map(int, f[1:]))
            out[int(f[0][3:])] = (sum(v) - v[3] - v[4], sum(v))
    return out


def busy(before, after, cpus):
    used = sum(after[c][0] - before[c][0] for c in cpus)
    total = sum(after[c][1] - before[c][1] for c in cpus)
    return round(len(cpus) * used / total, 3) if total else None


PROBE_SYMBOL = "CRALLOC_PROBE_COUNTS"


def probe_reader(pid, binary):
    """A function that reads the probe build's slow-path counters out of the server's memory,
    or None for a build without them (crates/cralloc/src/probe.rs)."""
    nm = subprocess.run(["nm", "-S", binary], capture_output=True, text=True).stdout
    sym = [l.split() for l in nm.splitlines() if l.endswith(" " + PROBE_SYMBOL)]
    if not sym:
        return None
    value, size = int(sym[0][0], 16), int(sym[0][1], 16)
    real = os.path.realpath(binary)
    base = None
    for line in Path(f"/proc/{pid}/maps").read_text().splitlines():
        f = line.split()
        if len(f) >= 6 and f[5] == real and int(f[2], 16) == 0:
            base = int(f[0].split("-")[0], 16)
            break
    if base is None:
        return None

    def read():
        with open(f"/proc/{pid}/mem", "rb") as m:
            m.seek(base + value)
            return struct.unpack(f"<{size // 8}Q", m.read(size))
    return read


def probe_delta(a, b):
    return {str(i): y - x for i, (x, y) in enumerate(zip(a, b)) if y != x}


def cstates(cpus=(0, 1, 2, 3)):
    """Entries into and time in each idle state, summed over the server's CPUs."""
    out = {}
    for c in cpus:
        base = Path(f"/sys/devices/system/cpu/cpu{c}/cpuidle")
        for st in sorted(base.glob("state*")):
            name = (st / "name").read_text().strip()
            u, t = out.get(name, (0, 0))
            out[name] = (u + int((st / "usage").read_text()), t + int((st / "time").read_text()))
    return out


def cstate_delta(a, b):
    return {k: {"entries": b[k][0] - a[k][0], "us": b[k][1] - a[k][1]} for k in a}


def parse_perf(path):
    vals, mux = {}, []
    for line in Path(path).read_text().splitlines():
        f = line.split(",")
        if len(f) < 5 or line.startswith("#"):
            continue
        try:
            vals[f[2]] = float(f[0])
        except ValueError:
            vals[f[2]] = None
        if f[4] and f[4] != "100.00":
            mux.append(f[2])
    vals["_multiplexed"] = mux
    return vals


def vm_hwm(pid):
    try:
        for line in Path(f"/proc/{pid}/status").read_text().splitlines():
            if line.startswith("VmHWM:"):
                return int(line.split()[1])
    except OSError:
        pass
    return None


def one_run(server, alloc, rnd, segs, outdir, wrap="", record=""):
    spec = SERVERS[server]
    # `build:NAME` is a native build of the server in bench/target/NAME, for experiments.
    a = ALLOCS[alloc] if alloc in ALLOCS else {"preload": "", "build": alloc.split(":", 1)[1], "env": {}}
    tag = f"{rnd:02d}-{alloc.replace(':', '-')}"
    d = outdir / server / tag
    d.mkdir(parents=True, exist_ok=True)
    env = dict(spec.get("env", {}), **a["env"])
    if spec.get("native"):
        binary = str(REPO / "bench" / "target" / a["build"] / "release" / "rpc-server")
    else:
        binary = a.get("bin") or spec["bin"]
        if a["preload"]:
            env["LD_PRELOAD"] = a["preload"]
    argv = [str(MEMLIMIT), "-m", "4G", "-t", "15m", "--"] + wrap.format(dir=d).split() \
        + ["taskset", "-c", SERVER_CPUS, "env"] + [f"{k}={v}" for k, v in env.items()] + [binary] + spec["args"]
    sout, serr = open(d / "server.out", "w+"), open(d / "server.err", "w")
    srv = subprocess.Popen(argv, stdout=sout, stderr=serr)
    rec = {"server": server, "alloc": alloc, "round": rnd, "t": time.time(), "dir": str(d)}
    perfs, pid = {}, None
    try:
        if spec.get("native"):
            port = None
            for _ in range(300):
                sout.seek(0)
                for line in sout.read().splitlines():
                    if line.startswith("BENCH_LISTEN"):
                        port = int(line.split(":")[-1])
                if port:
                    break
                time.sleep(0.02)
        else:
            port = spec["port"]
        if not port or not wait_port(port):
            raise RuntimeError("server did not listen")
        for _ in range(100):
            pid = find_server(srv.pid, binary)
            if pid:
                break
            time.sleep(0.01)
        if not pid:
            raise RuntimeError("server process not found")
        maps = Path(f"/proc/{pid}/maps").read_text()
        rec["preload_seen"] = (os.path.basename(a["preload"]) in maps) if (a["preload"] and not spec.get("native")) else None
        plan = ",".join(f"{n}:{k}:{s}" for n, k, s in segs)
        gen = [str(MEMLIMIT), "-m", "2G", "-t", "15m", "--", "taskset", "-c", CLIENT_CPUS, str(LOADGEN),
               "--addr", f"127.0.0.1:{port}", "--path", spec["path"], "--content-type", spec["ctype"],
               "--body", spec["body"], "--conns", "64", "--threads", "4", "--cpus", CLIENT_CPUS,
               "--aux-cpu", AUX_CPU, "--pid", str(pid), "--plan", plan, "--label", f"{server} {alloc} {rnd}",
               "--json", str(d / "summary.json"), "--raw", str(d / "raw.bin"), "--mem", str(d / "mem.csv")]
        for h in spec["headers"]:
            gen += ["--header", h]
        wins = windows(segs)
        starts = {w[1]: w for w in wins}
        stops = {}
        for w in wins:
            stops.setdefault(w[2], []).append(w[0])
        before = cpu_times()
        # Lines up wall-clock tools (strace -ttt) with the generator's monotonic timeline.
        rec["realtime_minus_monotonic_ns"] = time.time_ns() - time.monotonic_ns()
        g = subprocess.Popen(gen, stdout=subprocess.PIPE, stderr=open(d / "gen.err", "w"), text=True)
        result, marks = None, []
        idle_at, idle_states = {}, {}
        probe = probe_reader(pid, binary) if spec.get("native") else None
        probe_at, probe_counts = {}, {}
        for line in g.stdout:
            if line.startswith("MARK"):
                f = line.split()
                idx = len(segs) if f[2] == "end" else int(f[2])
                marks.append(line.strip())
                for label in stops.get(idx, []):
                    if label in perfs:
                        perfs[label][0].send_signal(signal.SIGINT)
                    if label in idle_at:
                        idle_states[label] = cstate_delta(idle_at.pop(label), cstates())
                    if label in probe_at:
                        probe_counts[label] = probe_delta(probe_at.pop(label), probe())
                if record and idx == 0:
                    p = subprocess.Popen(["taskset", "-c", AUX_CPU, "perf", "record", "-o", str(d / "perf.data")]
                                         + record.split() + ["-p", str(pid)],
                                         stdout=subprocess.DEVNULL, stderr=open(d / "perf-record.err", "w"))
                    perfs["record"] = (p, d / "perf.data", time.monotonic())
                if record and idx == len(segs):
                    perfs["record"][0].send_signal(signal.SIGINT)
                if idx in starts:
                    label = starts[idx][0]
                    out = d / f"perf-{label}.csv"
                    p = subprocess.Popen(["taskset", "-c", AUX_CPU, "perf", "stat", "-x,", "-o", str(out),
                                          "-e", EVENTS, "-p", str(pid)],
                                         stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
                    perfs[label] = (p, out, time.monotonic())
                    idle_at[label] = cstates()
                    if probe:
                        probe_at[label] = probe()
            elif line.startswith("RESULT "):
                result = json.loads(line[7:])
        g.wait()
        after = cpu_times()
        for label, (p, out, t0) in perfs.items():
            if p.poll() is None:
                p.send_signal(signal.SIGINT)
            p.wait(timeout=60)
        rec["perf"] = {label: parse_perf(out) for label, (p, out, t0) in perfs.items()
                       if label != "record" and out.exists()}
        rec["cstates"] = idle_states
        if probe_counts:
            rec["probe"] = probe_counts
        rec["gen_rc"] = g.returncode
        rec["summary"] = result
        rec["marks"] = marks
        rec["vm_hwm_kib"] = vm_hwm(pid)
        rec["load"] = {"server_siblings": busy(before, after, [16, 17, 18, 19]),
                       "client_siblings": busy(before, after, [24, 25, 26, 27]),
                       "others": busy(before, after, [4, 5, 6, 7, 20, 21, 22, 23, 12, 13, 14, 28, 29, 30, 31])}
    finally:
        if pid:
            try:
                os.kill(pid, signal.SIGTERM if spec.get("native") else signal.SIGINT)
            except ProcessLookupError:
                pass
        try:
            srv.wait(timeout=20)
        except subprocess.TimeoutExpired:
            if pid:
                try:
                    os.kill(pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
            srv.wait()
        for label, (p, out, t0) in perfs.items():
            if p.poll() is None:
                p.kill()
                p.wait()
    sout.seek(0)
    for line in sout.read().splitlines():
        if line.startswith("BENCH_RESULT"):
            rec["server_report"] = line[len("BENCH_RESULT"):].strip()
    rec["server_err_tail"] = (d / "server.err").read_text()[-400:]
    return rec


def cmd_run(args):
    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    allocs = args.allocs.split(",")
    if args.custom:
        segs = [tuple(s.split(":")) for s in args.custom.split(",")]
    elif args.plan == "sat":
        segs = sat_plan()
    else:
        r30, r70 = args.rates.split(",")
        segs = day_plan(r30, r70)
    if args.server == "rpc" and not (TARGET / "log50.bin").exists():
        with open(TARGET / "log50.bin", "wb") as f:
            subprocess.run([sys.executable, str(HERE / "connect_log_body.py"), "50"], stdout=f, check=True)
    rng = random.Random(args.seed)
    with open(out / "runs.jsonl", "a") as log:
        for rnd in range(args.first_round, args.first_round + args.rounds):
            order = allocs[:]
            rng.shuffle(order)
            for alloc in order:
                t0 = time.monotonic()
                try:
                    rec = one_run(args.server, alloc, rnd, segs, out, args.wrap, args.record)
                    rec["ok"] = rec.get("gen_rc") == 0 and rec.get("summary") is not None \
                        and rec.get("preload_seen") is not False
                except Exception as e:  # a failed run is recorded and the session goes on
                    rec = {"server": args.server, "alloc": alloc, "round": rnd, "ok": False, "error": repr(e)}
                rec["plan"] = args.plan
                log.write(json.dumps(rec) + "\n")
                log.flush()
                s = rec.get("summary") or {}
                sat = s.get("groups", {}).get("sat", {}).get("rate")
                print(f"{args.server} r{rnd} {alloc:9s} ok={rec['ok']} sat={sat} "
                      f"({time.monotonic() - t0:.0f} s)", flush=True)
                time.sleep(1)


# ---------------------------------------------------------------- report

def load(dirs):
    runs = []
    for d in dirs:
        for line in open(Path(d) / "runs.jsonl"):
            r = json.loads(line)
            if r.get("ok"):
                runs.append(r)
    return runs


def med(xs):
    xs = [x for x in xs if x is not None]
    return statistics.median(xs) if xs else None


def spread(xs):
    """Half the range over the median, a crude noise figure for a handful of rounds."""
    xs = [x for x in xs if x is not None]
    if len(xs) < 2 or not statistics.median(xs):
        return None
    return (max(xs) - min(xs)) / 2 / statistics.median(xs)


def fmt(x, nd=1):
    if x is None:
        return "-"
    if isinstance(x, float) and abs(x) >= 1000:
        return f"{x:,.0f}"
    return f"{x:.{nd}f}" if isinstance(x, float) else str(x)


def per_request(r, label, group):
    p = r.get("perf", {}).get(label)
    s = r["summary"]["groups"]
    n = sum(s.get(g, {}).get("lat_us", {}).get("n", 0) for g in group)
    if not p or not n:
        return {}
    return {"cyc": ((p.get("cycles:u") or 0) + (p.get("cycles:k") or 0)) / n,
            "kcyc": (p.get("cycles:k") or 0) / n,
            "cpu_us": (p.get("task-clock") or 0) * 1e3 / n,  # task-clock is in ms
            "faults": (p.get("page-faults") or 0) / n,
            "cs": (p.get("context-switches") or 0) / n,
            "mux": p.get("_multiplexed")}


def cmd_report(args):
    runs = load(args.dirs)
    servers = sorted({r["server"] for r in runs})
    for srv in servers:
        rs = [r for r in runs if r["server"] == srv]
        allocs = sorted({r["alloc"] for r in rs})
        print(f"\n## {srv}  ({len(rs)} runs)")
        print("alloc      rounds  sat req/s (noise)")
        for a in allocs:
            ra = [r for r in rs if r["alloc"] == a]
            sat = [r["summary"]["groups"].get("sat", {}).get("rate") for r in ra]
            print(f"{a:10s} {len(ra):6d}  {fmt(med(sat))} ({fmt((spread(sat) or 0) * 100)}%)")
        for g in ("s30", "s70", "on"):
            if not any(g in r["summary"]["groups"] for r in rs):
                continue
            print(f"\n{g}: latency in us (median over rounds; noise = half-range/median of each)")
            print("alloc      rate      p50    p90    p99   p99.9     max  missed  cyc/req  kcyc/req  cpu us/req  "
                  "faults/req  cs/req  rss MiB")
            for a in allocs:
                ra = [r for r in rs if r["alloc"] == a and g in r["summary"]["groups"]]
                gs = [r["summary"]["groups"][g] for r in ra]
                lat = {k: med([x["lat_us"][k] for x in gs]) for k in ("p50", "p90", "p99", "p999", "max")}
                noise = {k: spread([x["lat_us"][k] for x in gs]) for k in ("p99", "p999")}
                label = "onoff" if g == "on" else g
                pr = [per_request(r, label, ["on"] if g == "on" else [g]) for r in ra]
                rss = med([x.get("rss_kib", {}).get("mean") for x in gs])
                print(f"{a:10s} {fmt(med([x['rate'] for x in gs])):>8s} {fmt(lat['p50']):>6s} {fmt(lat['p90']):>6s} "
                      f"{fmt(lat['p99']):>6s} {fmt(lat['p999']):>7s} {fmt(lat['max']):>7s} "
                      f"{fmt(med([x['missed'] for x in gs])):>7s} "
                      f"{fmt(med([p.get('cyc') for p in pr]), 0):>8s} {fmt(med([p.get('kcyc') for p in pr]), 0):>9s} "
                      f"{fmt(med([p.get('cpu_us') for p in pr]), 2):>11s} {fmt(med([p.get('faults') for p in pr]), 4):>11s} "
                      f"{fmt(med([p.get('cs') for p in pr]), 3):>7s} {fmt(rss / 1024 if rss else None):>8s}"
                      f"   noise p99 {fmt((noise['p99'] or 0) * 100)}% p99.9 {fmt((noise['p999'] or 0) * 100)}%")
            if g == "on":
                print("\nafter silence: first {} ms of each burst against the rest".format(
                    rs[0]["summary"].get("head_ms")))
                print("alloc        head n   p50    p99   p99.9    max  |  rest p50   p99  p99.9    max")
                for a in allocs:
                    gs = [r["summary"]["groups"]["on"] for r in rs if r["alloc"] == a and "on" in r["summary"]["groups"]]
                    h = {k: med([x["head"][k] for x in gs]) for k in ("n", "p50", "p99", "p999", "max")}
                    t = {k: med([x["rest"][k] for x in gs]) for k in ("p50", "p99", "p999", "max")}
                    print(f"{a:10s} {fmt(h['n']):>8s} {fmt(h['p50']):>5s} {fmt(h['p99']):>6s} {fmt(h['p999']):>7s} "
                          f"{fmt(h['max']):>6s}  |  {fmt(t['p50']):>8s} {fmt(t['p99']):>5s} {fmt(t['p999']):>6s} "
                          f"{fmt(t['max']):>6s}")


IDLE_MARKS = (0.05, 0.25, 0.5, 1, 2, 3, 5, 10, 19.9)


def memory_curve(r):
    """Resident MiB at marks through the silences and after the connections close."""
    path = Path(r["dir"]) / "mem.csv"
    if not path.exists():
        return None
    rows = [tuple(map(float, l.split(","))) for l in path.read_text().splitlines()[1:] if l]
    segs = r["summary"]["segments"]

    def at(t_ms):
        best = None
        for t, rss, _ in rows:
            if t <= t_ms:
                best = rss
            else:
                break
        return best / 1024 if best is not None else None

    out = {}
    s70 = [s for s in segs if s["name"] == "s70"]
    if s70:
        out["s70_end"] = at(s70[0]["end_ms"] - 1)
    offs = [s for s in segs if s["name"] == "off"]
    if offs:
        # Each silence: at its start, at its end (just before the burst), and its lowest.
        out["off_start"] = statistics.mean(at(s["begin_ms"] + 5) for s in offs)
        out["off_end"] = statistics.mean(at(s["end_ms"] - 1) for s in offs)
        lows = []
        for s in offs:
            v = [rss for t, rss, _ in rows if s["begin_ms"] <= t < s["end_ms"]]
            lows.append(min(v) / 1024 if v else None)
        out["off_low"] = statistics.mean(x for x in lows if x is not None)
    ons = [s for s in segs if s["name"] == "on"]
    if ons:
        out["on_peak"] = statistics.mean(
            max([rss for t, rss, _ in rows if s["begin_ms"] <= t < s["end_ms"]] or [0]) / 1024 for s in ons)
    idle = [s for s in segs if s["name"] == "idle"]
    if idle:
        for m in IDLE_MARKS:
            out[f"idle+{m}s"] = at(idle[0]["begin_ms"] + m * 1000)
    return out


def cmd_memory(args):
    runs = load(args.dirs)
    for srv in sorted({r["server"] for r in runs}):
        rs = [r for r in runs if r["server"] == srv and r["summary"]]
        print(f"\n## {srv}: resident MiB (median over rounds; [min-max])")
        curves = {}
        for r in rs:
            c = memory_curve(r)
            if c:
                curves.setdefault(r["alloc"], []).append(c)
        keys = ["s70_end", "off_start", "off_low", "off_end", "on_peak"] + [f"idle+{m}s" for m in IDLE_MARKS]
        print("alloc     " + " ".join(f"{k:>14s}" for k in keys))
        for a, cs in sorted(curves.items()):
            cells = []
            for k in keys:
                v = [c.get(k) for c in cs if c.get(k) is not None]
                cells.append(f"{statistics.median(v):6.1f}[{min(v):.0f}-{max(v):.0f}]" if v else "-")
            print(f"{a:9s} " + " ".join(f"{c:>14s}" for c in cells))


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)
    r = sub.add_parser("run")
    r.add_argument("server", choices=sorted(SERVERS))
    r.add_argument("--plan", choices=["sat", "day"], required=True)
    r.add_argument("--rates", help="R30,R70 in requests per second, for --plan day")
    r.add_argument("--custom", help="a loadgen plan (NAME:KIND:SECS,...) in place of --plan's")
    r.add_argument("--rounds", type=int, default=1)
    r.add_argument("--first-round", type=int, default=1)
    r.add_argument("--allocs", default=DEFAULT_ALLOCS)
    r.add_argument("--wrap", default="", help="a command prefix for the server, {dir} the run's directory "
                   "(for example strace); it runs outside the server's CPU list unless it says otherwise")
    r.add_argument("--record", default="", help="perf record options; records the server from the first "
                   "segment to the end into {dir}/perf.data")
    r.add_argument("--seed", type=int, default=int(time.time()))
    r.add_argument("--out", required=True)
    p = sub.add_parser("report")
    p.add_argument("dirs", nargs="+")
    m = sub.add_parser("memory")
    m.add_argument("dirs", nargs="+")
    args = ap.parse_args()
    if args.cmd == "run":
        if args.plan == "day" and not args.rates and not args.custom:
            ap.error("--plan day needs --rates")
        cmd_run(args)
    elif args.cmd == "memory":
        cmd_memory(args)
    else:
        cmd_report(args)


if __name__ == "__main__":
    main()
