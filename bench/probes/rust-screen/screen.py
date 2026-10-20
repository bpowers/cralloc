#!/usr/bin/env python3
"""Screens a real program as an allocator benchmark.

  screen.py time NAME [--rounds 3]   glibc, jemalloc, mimalloc and asmalloc preloaded, the
                                     allocators in a new random order each round; time,
                                     cycles, instructions, faults and peak resident set.
  screen.py perf NAME                perf record under glibc: the share of cycles in glibc's
                                     allocator, in the kernel and in page faults.
  screen.py profile NAME             one run under alloc_profile.so (not timed).
  screen.py report [NAME...]         a table of what the three commands recorded.
  screen.py list                     the candidates and their commands.

Candidates are defined in candidates.py. Results go to ROOT/results/NAME/ as JSON lines.
ROOT is target/rust-screen of this checkout unless RUST_SCREEN_ROOT says otherwise; the
README beside this file says what lives there.

`time` and `perf` are measurements: run them under scripts/measure (with --both-l3 when the
candidate uses CPUs of both L3s) and scripts/memlimit. `profile` is not: run it under
scripts/heavy and scripts/memlimit. Preloading measures the C entry points only.
"""

import argparse
import hashlib
import json
import os
import random
import re
import shlex
import signal
import socket
import statistics
import subprocess
import sys
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[2]
ROOT = Path(os.environ.get("RUST_SCREEN_ROOT", REPO / "target" / "rust-screen"))


def main_checkout() -> Path:
    common = subprocess.run(["git", "-C", str(REPO), "rev-parse", "--path-format=absolute", "--git-common-dir"],
                            capture_output=True, text=True, check=True).stdout.strip()
    return Path(common).parent


BASE = Path(os.environ.get("BENCH_BASELINES", main_checkout() / "bench" / "baselines" / "out"))
PROBE = BASE / "probe" / "libbenchprobe.so"
ALLOCS = {
    "glibc": "",
    "jemalloc": str(BASE / "jemalloc" / "libjemalloc.so"),
    "mimalloc": str(BASE / "mimalloc" / "libmimalloc.so"),
    "asmalloc": str(ROOT / "lib" / "libasmalloc_preload.so"),
}
RECORDER = ROOT / "tools" / "liballoc_profile.so"
SPIN = ROOT / "tools" / "spin"
WALLTIME = ROOT / "tools" / "walltime"
EVENTS = "cycles:u,cycles:k,instructions:u,task-clock,page-faults"

sys.path.insert(0, str(HERE))
from candidates import CANDIDATES, INPUTS  # noqa: E402


def expand(s, spec):
    return s.format(root=ROOT, bin=ROOT / "bin", inp=ROOT / "inputs", work=ROOT / "work" / spec["name"])


def spec_of(name):
    if name not in CANDIDATES:
        sys.exit(f"unknown candidate {name}; `screen.py list` prints them")
    s = dict(CANDIDATES[name])
    s["name"] = name
    s.setdefault("cpus", "2")
    s.setdefault("env", {})
    s.setdefault("setup", [])
    s.setdefault("timeout", 120)
    return s


def outdir(name):
    d = ROOT / "results" / name
    d.mkdir(parents=True, exist_ok=True)
    return d


def setup(spec):
    work = ROOT / "work" / spec["name"]
    work.mkdir(parents=True, exist_ok=True)
    for line in spec["setup"]:
        subprocess.run(expand(line, spec), shell=True, check=True, cwd=work)
    return work


def argv_of(spec, key="cmd"):
    return [expand(a, spec) for a in spec[key]]


def target_env(spec, extra):
    env = [f"{k}={expand(v, spec)}" for k, v in spec["env"].items()]
    return env + [f"{k}={v}" for k, v in extra.items()]


def spin(cpus):
    subprocess.run([str(SPIN), cpus, "300"], check=True)


def wait_port(port, timeout=30.0):
    end = time.monotonic() + timeout
    while time.monotonic() < end:
        try:
            with socket.create_connection(("127.0.0.1", port), timeout=0.2):
                return True
        except OSError:
            time.sleep(0.02)
    return False


def child_of(pid):
    try:
        kids = Path(f"/proc/{pid}/task/{pid}/children").read_text().split()
        return int(kids[0]) if kids else None
    except OSError:
        return None


def vm_hwm(pid):
    try:
        for line in Path(f"/proc/{pid}/status").read_text().splitlines():
            if line.startswith("VmHWM:"):
                return int(line.split()[1]) * 1024
    except OSError:
        pass
    return None


def maps_has(pid, lib):
    try:
        return lib in Path(f"/proc/{pid}/maps").read_text()
    except OSError:
        return None


def run_server(spec, argv, cwd, out, err):
    """Starts the server under argv, drives it with the client, stops it. Returns
    (client wall seconds, server peak RSS, preload seen in maps, client output)."""
    srv = spec["server"]
    p = subprocess.Popen(argv, cwd=cwd, stdout=out, stderr=err)
    pid = None
    try:
        if not wait_port(srv["port"]):
            raise RuntimeError("server did not open its port")
        pid = None
        for _ in range(100):
            pid = child_of(p.pid) if argv[0] in ("perf",) else p.pid
            if pid:
                break
            time.sleep(0.01)
        client = ["taskset", "-c", srv.get("client_cpus", "8-11")] + [expand(a, spec) for a in srv["client"]]
        t0 = time.perf_counter()
        c = subprocess.run(client, cwd=cwd, capture_output=True, text=True, timeout=spec["timeout"])
        wall = time.perf_counter() - t0
        hwm = vm_hwm(pid)
        seen = {k: maps_has(pid, os.path.basename(v)) for k, v in ALLOCS.items() if v}
        os.kill(pid, srv.get("stop", signal.SIGINT))
        try:
            p.wait(timeout=20)
        except subprocess.TimeoutExpired:
            os.kill(pid, signal.SIGKILL)
            p.wait()
        return wall, hwm, seen, c.returncode, c.stdout[-4000:] + c.stderr[-2000:]
    finally:
        # The server is perf's child when perf runs it; stop it too, or it keeps the port.
        if pid and pid != p.pid and p.poll() is None:
            try:
                os.kill(pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
        if p.poll() is None:
            p.kill()
            p.wait()


def parse_stat(path):
    vals = {}
    for line in Path(path).read_text().splitlines():
        f = line.split(",")
        if len(f) < 3 or line.startswith("#"):
            continue
        name = f[2]
        try:
            v = float(f[0])
        except ValueError:
            v = None
        vals[name] = v
        if len(f) > 4 and f[4] and f[4] not in ("100.00",):
            vals.setdefault("_multiplexed", []).append(name)
    return vals


def time_one(spec, alloc, rnd, scratch):
    work = setup(spec)
    cpus = spec["cpus"]
    lib = ALLOCS[alloc]
    probe_out = scratch / "probe.json"
    probe_out.unlink(missing_ok=True)
    stat = scratch / "stat.csv"
    pre = f"{lib}:{PROBE}" if lib else str(PROBE)
    env = target_env(spec, {"LD_PRELOAD": pre, "BENCH_PROBE_OUT": str(probe_out)})
    key = "server_cmd" if "server" in spec else "cmd"
    wall_file = scratch / "wall"
    wall_file.unlink(missing_ok=True)
    timer = [] if "server" in spec else [str(WALLTIME), str(wall_file)]
    argv = ["perf", "stat", "-x,", "-o", str(stat), "-e", EVENTS, "--", "taskset", "-c", cpus] + timer + ["env"] \
        + env + argv_of(spec, key)
    out_path, err_path = scratch / "stdout", scratch / "stderr"
    spin(spec.get("spin_cpus", cpus))
    rec = {"name": spec["name"], "alloc": alloc, "round": rnd, "t": time.time()}
    with open(out_path, "wb") as out, open(err_path, "wb") as err:
        if "server" in spec:
            wall, hwm, seen, crc, ctext = run_server(spec, argv, work, out, err)
            rec.update(wall_s=wall, vm_hwm=hwm, maps=seen, client_rc=crc, client_tail=ctext[-600:])
            rc = 0 if crc == 0 else crc
        else:
            t0 = time.perf_counter()
            try:
                p = subprocess.run(argv, cwd=work, stdout=out, stderr=err, timeout=spec["timeout"])
                rc = p.returncode
            except subprocess.TimeoutExpired:
                rc = "timeout"
            rec["wall_outside_s"] = time.perf_counter() - t0
            try:
                rec["wall_s"] = float(wall_file.read_text())
            except (OSError, ValueError):
                rec["wall_s"] = rec["wall_outside_s"]
    rec["rc"] = rc
    try:
        rec["stat"] = parse_stat(stat)
    except OSError:
        rec["stat"] = {}
    try:
        probe = json.loads(probe_out.read_text())
        rec["probe_alloc"] = probe.get("allocator")
        if "vm_hwm" not in rec or rec["vm_hwm"] is None:
            rec["vm_hwm"] = probe.get("vm_hwm")
    except (OSError, ValueError):
        rec["probe_alloc"] = None
    data = out_path.read_bytes()
    # Sorted, so that a parallel program's output order does not count as a difference.
    rec["stdout_sha"] = hashlib.sha256(b"\n".join(sorted(data.split(b"\n")))).hexdigest()[:16]
    rec["stdout_len"] = len(data)
    rec["stderr_tail"] = err_path.read_bytes()[-600:].decode(errors="replace")
    ok = rc in spec.get("ok_rc", [0])
    if "server" not in spec and rec["probe_alloc"] != alloc:
        ok = False
    if "server" in spec and lib and not rec["maps"].get(alloc):
        ok = False
    rec["ok"] = ok
    return rec


def cmd_time(a):
    spec = spec_of(a.name)
    d = outdir(a.name)
    scratch = ROOT / "scratch" / a.name
    scratch.mkdir(parents=True, exist_ok=True)
    allocs = a.allocs.split(",")
    with open(d / "time.jsonl", "a") as f:
        for r in range(a.rounds):
            order = allocs[:]
            random.shuffle(order)
            for alloc in order:
                rec = time_one(spec, alloc, r, scratch)
                f.write(json.dumps(rec) + "\n")
                f.flush()
                st = rec["stat"]
                print(f"{a.name} r{r} {alloc:9} wall={rec['wall_s']:.3f}s cyc_u={st.get('cycles:u', 0) or 0:.3g} "
                      f"hwm={(rec.get('vm_hwm') or 0) / 2**20:.0f}MiB rc={rec['rc']} ok={rec['ok']} "
                      f"out={rec['stdout_sha']}", flush=True)
                if not rec["ok"]:
                    print("   stderr: " + rec["stderr_tail"][-300:].replace("\n", " | "), flush=True)


ALLOC_SYM = re.compile(r"malloc|free|calloc|realloc|memalign|_int_|tcache|unlink_chunk|consolidate|arena|"
                       r"heap_trim|systrim|sysmalloc|mremap_chunk|munmap_chunk|alloc_perturb|checked_request2size|"
                       r"malloc_usable|__libc_")
FAULT_SYMS = ["asm_exc_page_fault"]
MM_SYSCALLS = ["__x64_sys_munmap", "__x64_sys_madvise", "__x64_sys_mmap", "__x64_sys_mremap", "__x64_sys_brk",
               "__x64_sys_mprotect"]


def perf_report(data):
    """Self shares by object and symbol, and the children share of fault and mm syscall frames."""
    selfrep = subprocess.run(["perf", "report", "-i", str(data), "--no-children", "--sort", "dso,sym", "--stdio", "-q",
                              "-g", "none", "--percent-limit", "0"], capture_output=True, text=True).stdout
    by_dso, alloc, top = {}, 0.0, []
    for line in selfrep.splitlines():
        m = re.match(r"\s*([\d.]+)%\s+(\S+)\s+\[(.)\]\s+(.*)", line)
        if not m:
            continue
        pct, dso, _, sym = float(m.group(1)), m.group(2), m.group(3), m.group(4).strip()
        by_dso[dso] = by_dso.get(dso, 0.0) + pct
        if dso.startswith("libc.so") and ALLOC_SYM.search(sym):
            alloc += pct
        if len(top) < 25:
            top.append([pct, dso, sym])
    chrep = subprocess.run(["perf", "report", "-i", str(data), "--children", "--sort", "sym", "--stdio", "-q", "-g",
                            "none", "--percent-limit", "0"], capture_output=True, text=True).stdout
    children = {}
    for line in chrep.splitlines():
        m = re.match(r"\s*([\d.]+)%\s+([\d.]+)%\s+\[(.)\]\s+(\S+)", line)
        if m:
            children[m.group(4)] = float(m.group(1))
    kernel = sum(v for k, v in by_dso.items() if k.startswith("[kernel"))
    return {
        "alloc_share": alloc,
        "kernel_share": kernel,
        "fault_share": sum(children.get(s, 0.0) for s in FAULT_SYMS),
        "mm_syscall_share": sum(children.get(s, 0.0) for s in MM_SYSCALLS),
        "by_dso": dict(sorted(by_dso.items(), key=lambda kv: -kv[1])[:12]),
        "top": top,
    }


def cmd_perf(a):
    spec = spec_of(a.name)
    d = outdir(a.name)
    scratch = ROOT / "scratch" / a.name
    scratch.mkdir(parents=True, exist_ok=True)
    work = setup(spec)
    data = scratch / "perf.data"
    key = "server_cmd" if "server" in spec else "cmd"
    lib = ALLOCS[a.alloc]
    env = target_env(spec, {"LD_PRELOAD": lib} if lib else {})
    argv = ["perf", "record", "-q", "-F", "1999", "-g", "-o", str(data), "--", "taskset", "-c", spec["cpus"],
            "env"] + env + argv_of(spec, key)
    spin(spec.get("spin_cpus", spec["cpus"]))
    with open(scratch / "perf.stdout", "wb") as out, open(scratch / "perf.stderr", "wb") as err:
        if "server" in spec:
            run_server(spec, argv, work, out, err)
        else:
            subprocess.run(argv, cwd=work, stdout=out, stderr=err, timeout=spec["timeout"] * 2)
    rep = perf_report(data)
    rep["name"] = a.name
    rep["alloc"] = a.alloc
    rep["t"] = time.time()
    if lib:
        # Everything in the preloaded library is the allocator.
        rep["alloc_share"] = sum(v for k, v in rep["by_dso"].items() if k == os.path.basename(lib))
    # Runs under another allocator go to a file of their own, which `report` does not read.
    with open(d / ("perf.jsonl" if a.alloc == "glibc" else f"perf-{a.alloc}.jsonl"), "a") as f:
        f.write(json.dumps(rep) + "\n")
    print(f"{a.name}: alloc {rep['alloc_share']:.1f}% kernel {rep['kernel_share']:.1f}% faults "
          f"{rep['fault_share']:.1f}% mm syscalls {rep['mm_syscall_share']:.1f}%")
    for pct, dso, sym in rep["top"][:12]:
        print(f"   {pct:6.2f}% {dso:28} {sym}")
    data.unlink(missing_ok=True)


def cmd_profile(a):
    spec = spec_of(a.name)
    d = outdir(a.name)
    pdir = ROOT / "scratch" / a.name / "prof"
    pdir.mkdir(parents=True, exist_ok=True)
    for f in pdir.glob("*.json"):
        f.unlink()
    work = setup(spec)
    env = target_env(spec, {"LD_PRELOAD": str(RECORDER), "ALLOC_PROFILE_OUT": str(pdir / "p"),
                            "ALLOC_PROFILE_SIGTERM": "1"})
    key = "server_cmd" if "server" in spec else "cmd"
    argv = ["env"] + env + argv_of(spec, key)
    t0 = time.perf_counter()
    with open(pdir / "stdout", "wb") as out, open(pdir / "stderr", "wb") as err:
        if "server" in spec:
            spec = dict(spec)
            spec["server"] = dict(spec["server"], stop=signal.SIGTERM, client_cpus=os.environ.get("CLIENT_CPUS", "12-15"))
            run_server(spec, argv, work, out, err)
        else:
            subprocess.run(argv, cwd=work, stdout=out, stderr=err, timeout=spec["timeout"] * 10)
    wall = time.perf_counter() - t0
    files = [json.loads(f.read_text()) for f in pdir.glob("p.*.json")]
    if not files:
        sys.exit("no profile written")
    main = max(files, key=lambda p: p["allocs"] + p["frees"] + p["reallocs"])
    main["processes"] = len(files)
    main["profiled_wall_s"] = wall
    main["name"] = a.name
    main["t"] = time.time()
    with open(d / "profile.jsonl", "a") as f:
        f.write(json.dumps(main) + "\n")
    s = summarise(main)
    print(json.dumps(s, indent=1))


SIZE_GROUPS = [("<=16", 0, 4), ("17-64", 5, 6), ("65-256", 7, 8), ("257-1K", 9, 10), ("1K-4K", 11, 12),
               ("4K-32K", 13, 15), ("32K-256K", 16, 18), ("256K-1M", 19, 20), (">1M", 21, 48)]


def summarise(p):
    calls = p["allocs"] + p["frees"] + p["reallocs"]
    tick_ns = p["wall_ns"] / p["ticks"] if p["ticks"] else 0
    life = p["life"]
    freed = sum(life)

    def life_q(q):
        acc = 0
        for k, n in enumerate(life):
            acc += n
            if acc >= q * freed:
                return (2 ** k) * tick_ns
        return None

    groups = {}
    for label, lo, hi in SIZE_GROUPS:
        c = sum(p["size_count"][lo:hi + 1])
        groups[label] = round(100 * c / max(p["allocs"], 1), 1)
    gbytes = {}
    for label, lo, hi in SIZE_GROUPS:
        b = sum(p["size_bytes"][lo:hi + 1])
        gbytes[label] = round(100 * b / max(p["bytes_alloc"], 1), 1)
    ratio_labels = ["<=1", "<=1.25", "<=1.5", "<=2", "<=4", "<=16", ">16"]
    rs = max(p["reallocs"], 1)
    return {
        "name": p.get("name"),
        "threads": p["threads"],
        "processes": p.get("processes"),
        "calls": calls,
        "allocs": p["allocs"],
        "frees": p["frees"],
        "reallocs": p["reallocs"],
        "cross_free_pct": round(100 * (p["frees_cross"] + p["reallocs_cross"]) / max(p["frees"] + p["reallocs"], 1), 2),
        "realloc_pct_of_calls": round(100 * p["reallocs"] / max(calls, 1), 2),
        "realloc_moved_pct": round(100 * p["realloc_moved"] / rs, 1),
        "realloc_ratio_pct": {k: round(100 * v / rs, 1) for k, v in zip(ratio_labels, p["ratio"])},
        "aligned_pct": round(100 * p["aligned"] / max(p["allocs"], 1), 2),
        "mean_size": round(p["bytes_alloc"] / max(p["allocs"], 1), 1),
        "size_pct_by_count": groups,
        "size_pct_by_bytes": gbytes,
        "peak_live_mib": round(p["peak_live_bytes"] / 2 ** 20, 1),
        "peak_live_count": p["peak_live_count"],
        "live_at_exit_pct": round(100 * p["live_count_exit"] / max(p["allocs"], 1), 1),
        "life_median_ns": life_q(0.5),
        "life_p90_ns": life_q(0.9),
        "life_p99_ns": life_q(0.99),
        "life_under_1us_pct": round(100 * sum(n for k, n in enumerate(life) if 2 ** k * tick_ns <= 1000) / max(freed, 1), 1),
        "profiled_wall_s": round(p.get("profiled_wall_s", 0), 2),
    }


def load(path):
    if not path.exists():
        return []
    return [json.loads(line) for line in path.read_text().splitlines() if line.strip()]


def med(xs):
    xs = [x for x in xs if x is not None]
    return statistics.median(xs) if xs else None


def table_row(name):
    d = ROOT / "results" / name
    t = [r for r in load(d / "time.jsonl") if r["ok"]]
    bad = [r for r in load(d / "time.jsonl") if not r["ok"]]
    row = {"name": name, "runs": len(t), "invalid": len(bad)}
    by = {}
    for r in t:
        by.setdefault(r["alloc"], []).append(r)
    stats = {}
    for alloc, rs in by.items():
        walls = [r["wall_s"] for r in rs]
        stats[alloc] = {
            "n": len(rs),
            "wall": med(walls),
            "wall_noise": (max(walls) - min(walls)) / med(walls) / 2 if len(walls) > 1 else None,
            "cyc": med([(r["stat"].get("cycles:u") or 0) + (r["stat"].get("cycles:k") or 0) for r in rs]),
            "cyc_k": med([r["stat"].get("cycles:k") for r in rs]),
            "instr": med([r["stat"].get("instructions:u") for r in rs]),
            "cpu_ms": med([r["stat"].get("task-clock") for r in rs]),
            "faults": med([r["stat"].get("page-faults") for r in rs]),
            "hwm": med([r.get("vm_hwm") for r in rs]),
            "outs": sorted({r["stdout_sha"] for r in rs}),
        }
    row["by_alloc"] = stats
    walls = {k: v["wall"] for k, v in stats.items() if v["wall"]}
    hwms = {k: v["hwm"] for k, v in stats.items() if v["hwm"]}
    if walls:
        row["time_spread"] = max(walls.values()) / min(walls.values()) - 1
        row["fastest"] = min(walls, key=walls.get)
        row["slowest"] = max(walls, key=walls.get)
    if hwms:
        row["mem_spread"] = max(hwms.values()) / min(hwms.values()) - 1
        row["smallest"] = min(hwms, key=hwms.get)
    row["noise"] = max((v["wall_noise"] or 0) for v in stats.values()) if stats else None
    perf = load(d / "perf.jsonl")
    if perf:
        row["perf"] = {k: perf[-1][k] for k in ("alloc_share", "kernel_share", "fault_share", "mm_syscall_share")}
    prof = load(d / "profile.jsonl")
    if prof:
        s = summarise(prof[-1])
        g = stats.get("glibc", {}).get("wall")
        s["calls_per_s"] = s["calls"] / g if g else None
        row["profile"] = s
    return row


def cmd_report(a):
    names = a.names or sorted(p.name for p in (ROOT / "results").iterdir() if (p / "time.jsonl").exists())
    rows = [table_row(n) for n in names]
    if a.json:
        print(json.dumps(rows, indent=1))
        return
    for row in rows:
        print(f"== {row['name']}: {row['runs']} runs ok, {row['invalid']} invalid; time spread "
              f"{100 * row.get('time_spread', 0):.1f}% (fastest {row.get('fastest')}, slowest {row.get('slowest')}), "
              f"noise {100 * (row.get('noise') or 0):.1f}%, memory spread {100 * row.get('mem_spread', 0):.1f}% "
              f"(smallest {row.get('smallest')})")
        for alloc in ALLOCS:
            v = row["by_alloc"].get(alloc)
            if not v:
                continue
            print(f"   {alloc:9} n={v['n']} wall {v['wall']:.3f}s ±{100 * (v['wall_noise'] or 0):.1f}% "
                  f"cycles {v['cyc'] / 1e9:.2f}G (kernel {(v['cyc_k'] or 0) / 1e9:.2f}G) instr {(v['instr'] or 0) / 1e9:.2f}G "
                  f"cpu {(v['cpu_ms'] or 0) / 1e3:.2f}s faults {(v['faults'] or 0) / 1e3:.0f}k "
                  f"hwm {(v['hwm'] or 0) / 2**20:.0f}MiB outs={len(v['outs'])}")
        if "perf" in row:
            p = row["perf"]
            print(f"   perf: alloc {p['alloc_share']:.1f}% kernel {p['kernel_share']:.1f}% faults {p['fault_share']:.1f}% "
                  f"mm syscalls {p['mm_syscall_share']:.1f}%")
        if "profile" in row:
            s = row["profile"]
            print(f"   profile: threads {s['threads']} calls/s {(s['calls_per_s'] or 0) / 1e6:.2f}M cross {s['cross_free_pct']}% "
                  f"realloc {s['realloc_pct_of_calls']}% peak {s['peak_live_mib']}MiB/{s['peak_live_count']} "
                  f"life p50 {s['life_median_ns']} sizes {s['size_pct_by_count']}")


def build_of(spec):
    """The build (ROOT/bin/NAME) a candidate runs, from the first {bin}/NAME/ in its command."""
    text = " ".join(spec.get("server_cmd", []) + spec.get("cmd", []))
    m = re.search(r"\{bin\}/([^/]+)/", text)
    return m.group(1) if m else None


def ncpus(cpus):
    n = 0
    for part in cpus.split(","):
        lo, _, hi = part.partition("-")
        n += int(hi or lo) - int(lo) + 1 if lo else 0
    return n


# Versions of what is not built by build.sh or build-src.sh.
VERSIONS = {"axum": "axum 0.8.9", "actix": "actix-web 4.17.0", "cargo-resolve": "cargo 1.99.1"}


def cmd_table(a):
    """Markdown rows for the research note. Peak RSS is the measured process's own; a
    candidate run through a shell loop reports the shell's, which is left out."""
    builds = {}
    for b in load(ROOT / "builds.jsonl"):
        if b["status"] == 0:
            builds[b["name"]] = b
    names = a.names or sorted(p.name for p in (ROOT / "results").iterdir() if (p / "time.jsonl").exists())
    print("| Candidate | Version | Input | CPUs (threads) | glibc time | Time je / mi / asm | Noise | Cycles spread | "
          "glibc peak RSS | RSS je / mi / asm | Alloc share | Kernel (faults) | Calls/s | Cross-thread frees | "
          "Reallocs | Peak heap (blocks) | Build |")
    print("|" + "---|" * 17)
    for n in names:
        row = table_row(n)
        spec = spec_of(n) if n in CANDIDATES else {}
        bname = build_of(spec) or ""
        b = builds.get(bname, {})
        version = VERSIONS.get(n) or VERSIONS.get(bname) or b.get("version", "-")
        by = row["by_alloc"]
        g = by.get("glibc", {})

        def rel(key, alloc):
            v = by.get(alloc, {}).get(key)
            return f"{v / g[key]:.2f}" if v and g.get(key) else "-"

        times = " / ".join(rel("wall", x) for x in ("jemalloc", "mimalloc", "asmalloc"))
        looped = spec.get("cmd", [""])[0] == "sh" and "for " in " ".join(spec.get("cmd", []))
        mems = "-" if looped else " / ".join(rel("hwm", x) for x in ("jemalloc", "mimalloc", "asmalloc"))
        grss = "-" if looped else f"{(g.get('hwm') or 0) / 2**20:.0f} MiB"
        cycs = [v["cyc"] for v in by.values() if v.get("cyc")]
        cspread = f"{100 * (max(cycs) / min(cycs) - 1):.0f}%" if cycs else "-"
        perf = row.get("perf", {})
        prof = row.get("profile", {})
        calls = prof.get("calls_per_s")
        heap = f"{prof.get('peak_live_mib', '-')} MiB ({prof.get('peak_live_count', 0) / 1e6:.2f}M)" if prof else "-"
        build = f"{b['build_s']} s" if b else "-"
        print(f"| {n} | {version} | {INPUTS.get(n, '')} | {ncpus(spec.get('cpus', ''))} ({prof.get('threads', '-')}) | "
              f"{g.get('wall', 0):.2f} s | {times} | {100 * (row.get('noise') or 0):.1f}% | {cspread} | {grss} | {mems} | "
              f"{perf.get('alloc_share', 0):.1f}% | {perf.get('kernel_share', 0):.0f}% ({perf.get('fault_share', 0):.0f}%) | "
              f"{(calls or 0) / 1e6:.1f}M | {prof.get('cross_free_pct', '-')}% | {prof.get('realloc_pct_of_calls', '-')}% | "
              f"{heap} | {build} |")


def cmd_list(_a):
    for name in sorted(CANDIDATES):
        s = spec_of(name)
        cmd = " ".join(shlex.quote(x) for x in argv_of(s, "server_cmd" if "server" in s else "cmd"))
        print(f"{name:14} cpus={s['cpus']:10} {cmd}")


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)
    t = sub.add_parser("time")
    t.add_argument("name")
    t.add_argument("--rounds", type=int, default=3)
    t.add_argument("--allocs", default=",".join(ALLOCS))
    p = sub.add_parser("perf")
    p.add_argument("name")
    p.add_argument("--alloc", default="glibc", choices=list(ALLOCS))
    pr = sub.add_parser("profile")
    pr.add_argument("name")
    r = sub.add_parser("report")
    r.add_argument("names", nargs="*")
    r.add_argument("--json", action="store_true")
    sub.add_parser("list")
    tb = sub.add_parser("table")
    tb.add_argument("names", nargs="*")
    a = ap.parse_args()
    {"time": cmd_time, "perf": cmd_perf, "profile": cmd_profile, "report": cmd_report, "list": cmd_list,
     "table": cmd_table}[a.cmd](a)


if __name__ == "__main__":
    main()
