#!/usr/bin/env python3
"""Where a server's threads wait: offcpu.py DIR [--top N]

Reads DIR/perf.data from `record.py --mode sched` (a `context-switches` sample with the kernel
call chain at every switch out on the server's CPUs, and the switch records) and prints, for
the server's threads (every thread whose name is not the generator's, the runner's or a
kernel thread's), the time spent switched out, summed by thread kind (the thread's name with
digits dropped) and the reason: the first kernel frame of the chain that names a wait (a
futex, the address-space lock, a page or folio lock, I/O, epoll, io_uring, a sleep, an
unmap or advice), else the first frame that is not the scheduler's; a chain with no wait
frame and `preempt` in it is a preemption. A wait is measured from the switch out to the
thread's next switch in on any of the recorded CPUs; one still open at the end is left out.
"""
import argparse
import re
import subprocess
from collections import defaultdict
from pathlib import Path

WAITS = [
    ("mmap_lock", re.compile(r"rwsem_down_(read|write)_slowpath|lock_mm_and_find_vma|mmap_read_lock|mmap_write_lock|down_(read|write)_killable")),
    ("folio/page lock", re.compile(r"folio_wait_bit|__folio_lock|wait_on_page|folio_wait")),
    ("io", re.compile(r"io_schedule|blk_|filemap_read_folio|ext4_|xfs_")),
    ("futex", re.compile(r"futex")),
    ("epoll", re.compile(r"ep_poll|do_epoll_wait")),
    ("io_uring", re.compile(r"io_cqring_wait|io_uring")),
    ("sleep", re.compile(r"hrtimer_nanosleep|do_nanosleep|schedule_hrtimeout")),
    ("munmap/madvise", re.compile(r"__vm_munmap|do_madvise|unmap_region|tlb_finish_mmu")),
]
EXCLUDE = re.compile(r"^(loadgen|benchrun|perf|swapper|kworker|ksoftirqd|migration|rcu|cpuhp|idle_inject|spin|irq/|:-1)")
SCHED = re.compile(r"^(__schedule|schedule|preempt_schedule|__cond_resched|context_switch|finish_task_switch|__perf_|perf_)")


def kind(comm):
    return re.sub(r"[0-9]+", "#", comm)


def reason(frames):
    for name, pat in WAITS:
        if any(pat.search(f) for f in frames):
            return name
    if any("preempt" in f or "irqentry_exit" in f or "cond_resched" in f for f in frames):
        return "preempted"
    for f in frames:
        if not SCHED.match(f):
            return f
    return "?"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("dir")
    ap.add_argument("--top", type=int, default=25)
    a = ap.parse_args()
    d = Path(a.dir)
    text = subprocess.run(
        ["perf", "script", "-i", str(d / "perf.data"), "--show-switch-events", "-F", "comm,tid,time,event,ip,sym"],
        capture_output=True, text=True, check=True).stdout
    head = re.compile(r"^\s*(.*?)\s+(\d+)\s+(\d+\.\d+):\s*(.*)$")
    out_at = {}
    tot = defaultdict(float)
    cnt = defaultdict(int)
    cur = None
    first = last = None
    for line in text.splitlines():
        m = head.match(line)
        if m and (":" in line.split()[min(2, len(line.split()) - 1)] or "PERF_RECORD" in line):
            comm, tid, t, rest = m.group(1), int(m.group(2)), float(m.group(3)), m.group(4)
            first = t if first is None else first
            last = t
            if "PERF_RECORD_SWITCH" in rest:
                cur = None
                if " IN " in f" {rest} " or rest.split()[1:2] == ["IN"]:
                    o = out_at.pop(tid, None)
                    if o is not None:
                        key = (kind(o[1]), reason(o[2]))
                        tot[key] += t - o[0]
                        cnt[key] += 1
                continue
            if "context-switches" in rest or "cs" in rest.split()[:2]:
                if EXCLUDE.match(comm) or tid == 0:
                    cur = None
                    continue
                cur = [t, comm, []]
                out_at[tid] = cur
                continue
            cur = None
        elif cur is not None and line.strip():
            parts = line.split()
            if len(parts) >= 2:
                cur[2].append(parts[1].split("+")[0])
    span = (last - first) if first is not None else 0
    print(f"{d}: {sum(cnt.values())} waits over {span:.2f} s")
    by_kind = defaultdict(float)
    for (k, r), v in tot.items():
        by_kind[k] += v
    print("  thread kind: seconds off CPU (summed over threads)")
    for k, v in sorted(by_kind.items(), key=lambda kv: -kv[1])[:12]:
        print(f"    {k:28s} {v:8.3f}")
    print("  thread kind, wait: seconds, count")
    for key, v in sorted(tot.items(), key=lambda kv: -kv[1])[: a.top]:
        k, r = key
        print(f"    {k:28s} {r:32s} {v:8.3f}  {cnt[key]}")


if __name__ == "__main__":
    main()
