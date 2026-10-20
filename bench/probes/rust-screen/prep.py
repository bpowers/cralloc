#!/usr/bin/env python3
"""Derives the screening inputs that are generated rather than downloaded, from what
inputs.sh fetched. Deterministic: the same downloads give the same files.

  prep.py [WHAT...]   docs, markdown, typst, python, octane, jscorpus (default: all)
"""
import json
import os
import shutil
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = Path(os.environ.get("RUST_SCREEN_ROOT", HERE.parents[2] / "target" / "rust-screen"))
INP = ROOT / "inputs"


def text_files():
    """Linux and CPython documentation sources, in a fixed order."""
    files = sorted((INP / "linux-6.12" / "Documentation").rglob("*.rst"))
    files += sorted((INP / "Python-3.13.0" / "Doc").rglob("*.rst"))
    return files


def docs():
    out = INP / "tantivy" / "docs.jsonl"
    out.parent.mkdir(parents=True, exist_ok=True)
    # Documentation, then about 260 MB of kernel source, so that indexing takes seconds.
    sources = text_files()
    for sub in ["fs", "net", "kernel", "mm", "drivers/net"]:
        sources += sorted(p for p in (INP / "linux-6.12" / sub).rglob("*") if p.suffix in (".c", ".h"))
    with open(out, "w") as f:
        for p in sources:
            body = p.read_text(errors="replace")
            f.write(json.dumps({"title": str(p.relative_to(INP)), "body": body}) + "\n")
    print(out, out.stat().st_size)
    # The schema `tantivy new` would write after its interactive questions: both fields indexed
    # with positions, the title stored.
    text = lambda name, stored: {"name": name, "type": "text", "options": {
        "indexing": {"record": "position", "fieldnorms": True, "tokenizer": "default"}, "stored": stored, "fast": False}}
    meta = {"index_settings": {"docstore_compression": "lz4", "docstore_blocksize": 16384}, "segments": [],
            "schema": [text("title", True), text("body", False)], "opstamp": 0}
    (out.parent / "meta.json").write_text(json.dumps(meta))


def markdown():
    out = INP / "md" / "corpus.md"
    out.parent.mkdir(parents=True, exist_ok=True)
    with open(out, "w") as f:
        for _ in range(3):
            for p in text_files():
                f.write(p.read_text(errors="replace"))
                f.write("\n\n")
    print(out, out.stat().st_size)


def typst():
    """A long report: headings, paragraphs of documentation text, lists, tables, equations and
    figures with cross-references, over about 1500 pages."""
    out = INP / "typst" / "doc.typ"
    out.parent.mkdir(parents=True, exist_ok=True)
    paras = []
    for p in text_files()[:600]:
        for block in p.read_text(errors="replace").split("\n\n"):
            b = " ".join(block.split())
            if 200 < len(b) < 1500 and all(c not in b for c in "#*_`$<>@[]\\=/~-+"):
                paras.append(b)
    lines = ['#set page(paper: "a4", numbering: "1")', '#set heading(numbering: "1.1")',
             '#set par(justify: true)', "#outline()", ""]
    k = 0
    for ch in range(1, 61):
        lines.append(f"= Chapter {ch}")
        for sec in range(1, 9):
            lines.append(f"== Section {ch}.{sec} <s{ch}-{sec}>")
            for _ in range(4):
                lines.append(paras[k % len(paras)])
                lines.append("")
                k += 1
            lines.append(f"$ sum_(i=1)^{ch + sec} x_i^{sec} = integral_0^{ch} f(t) dif t + "
                         f"mat(1, {ch}; {sec}, 1) $")
            lines.append("")
            cells = ", ".join(f"[{r * c + ch}]" for r in range(1, 9) for c in range(1, 5))
            lines.append(f"#figure(table(columns: 4, {cells}), caption: [Table {ch}.{sec}]) <t{ch}-{sec}>")
            lines.append("")
            lines.append("- " + paras[(k + 1) % len(paras)][:200])
            lines.append("- see @s" + f"{ch}-{sec} and @t{ch}-{sec}")
            lines.append("")
    out.write_text("\n".join(lines))
    print(out, out.stat().st_size)


PY = r'''
# A mixed workload for a Python interpreter: Richards (pyperformance's, object-heavy), string
# building and parsing, dict and list churn, and recursion over small tuples.
import sys
sys.setrecursionlimit(5000)

I_IDLE, I_WORK, I_HANDLERA, I_HANDLERB, I_DEVA, I_DEVB = 1, 2, 3, 4, 5, 6
K_DEV, K_WORK = 1000, 1001
BUFSIZE = 4

class Packet:
    def __init__(self, l, i, k):
        self.link = l; self.ident = i; self.kind = k; self.datum = 0; self.data = [0] * BUFSIZE
    def append_to(self, lst):
        self.link = None
        if lst is None:
            return self
        p = lst; nxt = p.link
        while nxt is not None:
            p = nxt; nxt = p.link
        p.link = self
        return lst

class TaskRec: pass
class DeviceTaskRec(TaskRec):
    def __init__(self): self.pending = None
class IdleTaskRec(TaskRec):
    def __init__(self): self.control = 1; self.count = 10000
class HandlerTaskRec(TaskRec):
    def __init__(self): self.work_in = None; self.device_in = None
    def workInAdd(self, p): self.work_in = p.append_to(self.work_in); return self.work_in
    def deviceInAdd(self, p): self.device_in = p.append_to(self.device_in); return self.device_in
class WorkerTaskRec(TaskRec):
    def __init__(self): self.destination = I_HANDLERA; self.count = 0

class TaskState:
    def __init__(self): self.packet_pending = True; self.task_waiting = False; self.task_holding = False
    def packetPending(self): self.packet_pending = True; self.task_waiting = False; self.task_holding = False; return self
    def waiting(self): self.packet_pending = False; self.task_waiting = True; self.task_holding = False; return self
    def running(self): self.packet_pending = False; self.task_waiting = False; self.task_holding = False; return self
    def waitingWithPacket(self): self.packet_pending = True; self.task_waiting = True; self.task_holding = False; return self
    def isPacketPending(self): return self.packet_pending
    def isTaskWaiting(self): return self.task_waiting
    def isTaskHolding(self): return self.task_holding
    def isTaskHoldingOrWaiting(self): return self.task_holding or (not self.packet_pending and self.task_waiting)
    def isWaitingWithPacket(self): return self.packet_pending and self.task_waiting and not self.task_holding

tracing = False
layout = 0
class TaskWorkArea:
    def __init__(self): self.taskTab = [None] * 10; self.taskList = None; self.holdCount = 0; self.qpktCount = 0
taskWorkArea = TaskWorkArea()

class Task(TaskState):
    def __init__(self, i, p, w, initialState, r):
        self.link = taskWorkArea.taskList; self.ident = i; self.priority = p; self.input = w
        self.packet_pending = initialState.isPacketPending(); self.task_waiting = initialState.isTaskWaiting()
        self.task_holding = initialState.isTaskHolding(); self.handle = r
        taskWorkArea.taskList = self; taskWorkArea.taskTab[i] = self
    def fn(self, pkt, r): raise NotImplementedError
    def addPacket(self, p, old):
        if self.input is None:
            self.input = p; self.packet_pending = True
            if self.priority > old.priority: return self
        else:
            p.append_to(self.input)
        return old
    def runTask(self):
        if self.isWaitingWithPacket():
            msg = self.input; self.input = msg.link
            if self.input is None: self.running()
            else: self.packetPending()
        else:
            msg = None
        return self.fn(msg, self.handle)
    def waitTask(self): self.task_waiting = True; return self
    def hold(self): taskWorkArea.holdCount += 1; self.task_holding = True; return self.link
    def release(self, i):
        t = self.findtcb(i); t.task_holding = False
        if t.priority > self.priority: return t
        return self
    def qpkt(self, pkt):
        t = self.findtcb(pkt.ident); taskWorkArea.qpktCount += 1; pkt.link = None; pkt.ident = self.ident
        return t.addPacket(pkt, self)
    def findtcb(self, id):
        t = taskWorkArea.taskTab[id]
        if t is None: raise Exception("Bad task id %d" % id)
        return t

class DeviceTask(Task):
    def fn(self, pkt, r):
        d = r
        if pkt is None:
            pkt = d.pending
            if pkt is None: return self.waitTask()
            d.pending = None; return self.qpkt(pkt)
        d.pending = pkt; return self.hold()

class HandlerTask(Task):
    def fn(self, pkt, r):
        h = r
        if pkt is not None:
            if pkt.kind == K_WORK: h.workInAdd(pkt)
            else: h.deviceInAdd(pkt)
        work = h.work_in
        if work is None: return self.waitTask()
        count = work.datum
        if count >= BUFSIZE:
            h.work_in = work.link; return self.qpkt(work)
        dev = h.device_in
        if dev is None: return self.waitTask()
        h.device_in = dev.link; dev.datum = work.data[count]; work.datum = count + 1
        return self.qpkt(dev)

class IdleTask(Task):
    def fn(self, pkt, r):
        i = r; i.count -= 1
        if i.count == 0: return self.hold()
        elif i.control & 1 == 0:
            i.control //= 2; return self.release(I_DEVA)
        else:
            i.control = i.control // 2 ^ 0xd008; return self.release(I_DEVB)

A = ord('A')
class WorkTask(Task):
    def fn(self, pkt, r):
        w = r
        if pkt is None: return self.waitTask()
        if w.destination == I_HANDLERA: dest = I_HANDLERB
        else: dest = I_HANDLERA
        w.destination = dest; pkt.ident = dest; pkt.datum = 0
        for i in range(BUFSIZE):
            w.count += 1
            if w.count > 26: w.count = 1
            pkt.data[i] = A + w.count - 1
        return self.qpkt(pkt)

def schedule():
    t = taskWorkArea.taskList
    while t is not None:
        if t.isTaskHoldingOrWaiting(): t = t.link
        else: t = t.runTask()

def richards(iterations):
    for _ in range(iterations):
        taskWorkArea.holdCount = 0; taskWorkArea.qpktCount = 0
        IdleTask(I_IDLE, 1, 10000, TaskState().running(), IdleTaskRec())
        wkq = Packet(None, 0, K_WORK); wkq = Packet(wkq, 0, K_WORK)
        WorkTask(I_WORK, 1000, wkq, TaskState().waitingWithPacket(), WorkerTaskRec())
        wkq = Packet(None, I_DEVA, K_DEV); wkq = Packet(wkq, I_DEVA, K_DEV); wkq = Packet(wkq, I_DEVA, K_DEV)
        HandlerTask(I_HANDLERA, 2000, wkq, TaskState().waitingWithPacket(), HandlerTaskRec())
        wkq = Packet(None, I_DEVB, K_DEV); wkq = Packet(wkq, I_DEVB, K_DEV); wkq = Packet(wkq, I_DEVB, K_DEV)
        HandlerTask(I_HANDLERB, 3000, wkq, TaskState().waitingWithPacket(), HandlerTaskRec())
        DeviceTask(I_DEVA, 4000, None, TaskState().waiting(), DeviceTaskRec())
        DeviceTask(I_DEVB, 5000, None, TaskState().waiting(), DeviceTaskRec())
        schedule()
    return taskWorkArea.holdCount, taskWorkArea.qpktCount

def strings(n):
    words = {}
    for i in range(n):
        line = ",".join(str(i * j % 9973) for j in range(12))
        for w in line.split(","):
            words[w] = words.get(w, 0) + 1
        s = ("%d:%s" % (i, line)).upper().replace("1", "one")
        if len(s) < 0: print(s)
    return len(words), sum(words.values())

def trees(depth):
    def make(d):
        return (None, None) if d == 0 else (make(d - 1), make(d - 1))
    def check(t):
        return 1 if t[0] is None else 1 + check(t[0]) + check(t[1])
    total = 0
    for d in range(4, depth + 1, 2):
        iters = 1 << (depth - d + 4)
        for _ in range(iters):
            total += check(make(d))
    return total

def churn(n):
    live = {}
    acc = 0
    for i in range(n):
        live[i] = [i, str(i), {"k": i}]
        if i >= 1000:
            acc += len(live.pop(i - 1000)[1])
    return acc, len(live)

print(richards(SCALE_R))
print(strings(SCALE_S))
print(trees(SCALE_T))
print(churn(SCALE_C))
'''


def python():
    out = INP / "py" / "bench.py"
    out.parent.mkdir(parents=True, exist_ok=True)
    src = PY.replace("SCALE_R", "5").replace("SCALE_S", "15000").replace("SCALE_T", "12").replace("SCALE_C", "100000")
    out.write_text(src)
    print(out)


OCTANE = ["richards", "deltablue", "raytrace", "splay", "earley-boyer", "navier-stokes", "crypto"]
# Fixed work, not fixed time: by default base.js runs each benchmark for a second, which would
# make the work, and so the allocations, depend on the allocator's speed.
RUNNER = r'''
BenchmarkSuite.config.doDeterministic = true;
BenchmarkSuite.config.doWarmup = false;
// Octane's iteration counts are sized for JITs; an interpreter needs fewer.
BenchmarkSuite.suites.forEach(function(s) {
  s.benchmarks.forEach(function(b) { b.deterministicIterations = Math.ceil(b.deterministicIterations / OCTANE_DIV); });
});
var __results = [];
BenchmarkSuite.RunSuites({
  NotifyResult: function(name, result) { __results.push(name); },
  NotifyError: function(name, error) { __results.push(name + " error " + error); },
  NotifyScore: function(score) { __results.push("done"); }
});
console.log(__results.join(" "));
'''


def octane():
    out = INP / "octane-run"
    out.mkdir(parents=True, exist_ok=True)
    base = (INP / "octane" / "base.js").read_text()
    for b in OCTANE:
        src = base + "\n" + (INP / "octane" / f"{b}.js").read_text() + "\n" + RUNNER.replace("OCTANE_DIV", "4")
        (out / f"{b}.js").write_text(src)
    print(out)


def jscorpus():
    out = INP / "jscorpus"
    if out.exists():
        shutil.rmtree(out)
    out.mkdir(parents=True)
    picks = {
        "typescript.js": "typescript-5.6.3/package/lib/typescript.js",
        "three.module.js": "three-0.170.0/package/build/three.module.js",
        "lodash.js": "lodash-4.17.21/package/lodash.js",
        "react-dom.development.js": "react-dom-18.3.1/package/cjs/react-dom.development.js",
        "vue.global.js": "vue-3.5.12/package/dist/vue.global.js",
        "d3.js": "d3-7.9.0/package/dist/d3.js",
        "moment.js": "moment-2.30.1/package/moment.js",
        "jquery.js": "jquery-3.7.1/package/dist/jquery.js",
    }
    for name, rel in picks.items():
        shutil.copy(INP / "js" / rel, out / name)
    print(out, sum(p.stat().st_size for p in out.iterdir()))


RHAI = r'''
// Object maps, arrays, strings and closures: the shapes a rhai script of game or config logic
// builds.
fn make_item(i) {
    #{ id: i, name: "item-" + i, tags: ["t" + (i % 7), "c" + (i % 3)], price: i * 37 % 1000 }
}
let total = 0;
let names = [];
for round in 0..ROUNDS {
    let items = [];
    for i in 0..2000 { items.push(make_item(i + round)); }
    let cheap = items.filter(|x| x.price < 500);
    let ids = cheap.map(|x| x.id * 2);
    total += ids.reduce(|sum, v| sum + v, 0);
    let s = "";
    for x in cheap { if x.id % 50 == 0 { s += x.name + ";"; } }
    names.push(s.len);
    let index = #{};
    for x in items { index[x.name] = x.tags; }
    total += index.len();
}
print(total);
print(names.len());
'''

STAR = r'''
# Dicts, lists, strings and comprehensions, as a Bazel-like build file evaluation makes them.
def make_target(i):
    return {
        "name": "target_%d" % i,
        "srcs": ["src/%d/file_%d.cc" % (i % 50, j) for j in range(8)],
        "deps": [":target_%d" % d for d in range(max(0, i - 4), i)],
        "copts": ["-O2", "-DID=%d" % i],
    }

def run(rounds):
    total = 0
    for r in range(rounds):
        targets = [make_target(i + r) for i in range(3000)]
        by_name = {t["name"]: t for t in targets}
        for t in targets:
            closure = {}
            for d in t["deps"]:
                if d[1:] in by_name:
                    closure[d] = len(by_name[d[1:]]["srcs"])
            total += len(closure) + len(",".join(t["srcs"]))
    return total

print(run(ROUNDS))
'''


def scripts():
    for name, src, rounds in [("rhai/bench.rhai", RHAI, "400"), ("starlark/bench.star", STAR, "300")]:
        out = INP / name
        out.parent.mkdir(parents=True, exist_ok=True)
        out.write_text(src.replace("ROUNDS", rounds))
        print(out)


def tpch():
    """One SQL script for datafusion-cli: the SF1 tables from tpchgen-cli as external Parquet
    tables, then DataFusion's 22 TPC-H queries (fetched at tag 54.1.0) in order."""
    d = INP / "tpch"
    lines = []
    for t in ["customer", "lineitem", "nation", "orders", "part", "partsupp", "region", "supplier"]:
        lines.append(f"CREATE EXTERNAL TABLE {t} STORED AS PARQUET LOCATION '{d / 'sf1' / (t + '.parquet')}';")
    for i in range(1, 23):
        lines.append((d / "queries" / f"q{i}.sql").read_text().strip())
    (d / "run.sql").write_text("\n".join(lines) + "\n")
    print(d / "run.sql")


STEPS = {"scripts": scripts, "docs": docs, "markdown": markdown, "typst": typst, "python": python,
         "octane": octane, "jscorpus": jscorpus, "tpch": tpch}

if __name__ == "__main__":
    for w in sys.argv[1:] or list(STEPS):
        STEPS[w]()
