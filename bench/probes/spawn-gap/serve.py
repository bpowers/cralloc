#!/usr/bin/env python3
"""Start the orders server on CPUs 0-3, drive it from 8-11 with loadgen, optionally under
perf c2c, then stop it.

usage: serve.py BUILD MODE PLAN [OUT]
  BUILD  bench/target/<BUILD>/apps/orders/release/orders
  MODE   axum or axum-h2 (HTTP/2, 8 conns x 8 streams)
  PLAN   loadgen plan
  env: TRACE=path (allocation trace), C2C=path (perf c2c record of CPUs 0-3 during the plan),
       SHAPE_* passed through, PSTAT=path (perf stat -C 0-3 over the whole plan)
"""
import os, subprocess, sys, time, signal

WT = str(__import__("pathlib").Path(__file__).resolve().parents[3])
build, mode, plan = sys.argv[1], sys.argv[2], sys.argv[3]
binp = f"{WT}/bench/target/{build}/apps/orders/release/orders"
env = {k: v for k, v in os.environ.items() if k in ("PATH", "HOME", "USER", "LANG", "DBUS_SESSION_BUS_ADDRESS", "XDG_RUNTIME_DIR") or k.startswith("SHAPE_")}
env["BENCH_CPUS"] = "0-3"
env["MEMLIMIT_QUIET"] = "1"
if os.environ.get("TRACE"):
    env["BENCH_ALLOC_TRACE"] = os.environ["TRACE"]
srv = subprocess.Popen([f"{WT}/scripts/memlimit", "-m", "4G", "-t", "300", "--", "taskset", "-c", "0-3", binp, "axum"],
                       env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
addr = None
for line in srv.stdout:
    if line.startswith("BENCH_LISTEN"):
        addr = line.split()[1]
        break
if not addr:
    print("no listen", srv.stderr.read()[-2000:])
    sys.exit(1)
# the server's own pid: the child of timeout inside the scope
time.sleep(0.2)
pids = subprocess.run(["pgrep", "-f", binp], capture_output=True, text=True).stdout.split()
spid = [p for p in pids if open(f"/proc/{p}/comm").read().strip() == "orders"]
spid = spid[0] if spid else None
lg = [f"{WT}/bench/target/loadgen/release/loadgen", "--addr", addr, "--path", "/orders",
      "--content-type", "application/json", "--body", f"{WT}/bench/apps/orders/order.json",
      "--threads", "4", "--cpus", "8-11", "--plan", plan, "--label", "x"]
if mode == "axum-h2":
    lg += ["--proto", "h2", "--streams", "8", "--conns", "8"]
else:
    lg += ["--conns", "64"]
perf = None
if os.environ.get("C2C"):
    perf = subprocess.Popen(["perf", "c2c", "record", "-o", os.environ["C2C"], "--", "-a", "-C", "0-3", "-c", "10000",
                             "sleep", os.environ.get("C2C_SECS", "6")], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
if os.environ.get("PSTAT") and spid:
    perf = subprocess.Popen(["perf", "stat", "-x", ",", "-o", os.environ["PSTAT"], "-p", spid, "-e",
                             os.environ.get("PSTAT_EVENTS", "cycles,instructions")], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
g = subprocess.run(lg, env={"PATH": os.environ["PATH"]}, capture_output=True, text=True, timeout=200)
if perf:
    if os.environ.get("PSTAT"):
        perf.send_signal(signal.SIGINT)
    perf.wait(timeout=60)
res = [l for l in g.stdout.splitlines() if l.startswith("RESULT")]
print(res[0][:3000] if res else ("loadgen failed: " + g.stderr[-1500:]))
if spid:
    os.kill(int(spid), signal.SIGTERM)
out, err = srv.communicate(timeout=60)
print([l for l in out.splitlines() if l.startswith("BENCH_RESULT")][:1])
