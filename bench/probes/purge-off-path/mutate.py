#!/usr/bin/env python3
"""The review's mutations of the purge-off-path branch, with the patterns moved where the code
moved (the thread's turn is `depot::thread`'s `Cache::thread_turn`). Apply one mutation at a time to the branch, run the library tests, report which fail.
Usage: mutate.py <worktree> [name ...]   (restores each file after its run)"""
import subprocess, sys, os
wt = sys.argv[1]
only = sys.argv[2:]
B = 'crates/asmalloc/src/back/'
M = {
 'purge_due_never': (B+'purge.rs',
   "        if seen & ACTIVE == 0 && state.growth.load(SeqCst) >= step || self.pass_published() {\n            return true;\n        }\n        let time = now();",
   "        if true {\n            let _ = (seen, step, now);\n            return false;\n        }\n        let time = now();"),
 'purge_due_no_hold': (B+'purge.rs',
   "        self.hold_due(time) && !self.silent(time)\n", "        let _ = time;\n        false\n"),
 'purge_due_no_growth': (B+'purge.rs',
   "        if seen & ACTIVE == 0 && state.growth.load(SeqCst) >= step || self.pass_published() {\n            return true;\n        }\n        let time = now();",
   "        let _ = step;\n        if self.pass_published() {\n            return true;\n        }\n        let time = now();"),
 'hold_arm_noop': (B+'hold.rs',
   "        if next.load(SeqCst) == 0 {\n            let at = NEXT_AT | u64::from(count.wrapping_add(HOLD_STEPS));",
   "        if false && next.load(SeqCst) == 0 {\n            let at = NEXT_AT | u64::from(count.wrapping_add(HOLD_STEPS));"),
 'plan_drops_armed': (B+'hold.rs',
   "            let next = sooner(seen, planned);",
   "            let next = if seen == 0 {\n                planned\n            } else {\n                let _ = sooner(seen, planned);\n                hold.next.store(planned, SeqCst);\n                return;\n            };"),
 'plan_no_clear': (B+'hold.rs',
   "        hold.next.swap(0, SeqCst);\n        let planned",
   "        let planned"),
 'no_heartbeat_payment': (B+'purge.rs',
   "        if self.state.growth.load(Relaxed) >= growth_unpaid(geometry) {",
   "        if false && self.state.growth.load(Relaxed) >= growth_unpaid(geometry) {"),
 'hold_level_noop': (B+'hold.rs',
   "        while seen == NEXT_LEVEL || seen == 0 {\n            let at = NEXT_AT | u64::from(hold.fine.count());",
   "        while false && (seen == NEXT_LEVEL || seen == 0) {\n            let at = NEXT_AT | u64::from(hold.fine.count());"),
 'held_region_not_next_step': (B+'hold.rs',
   "                    // at the next step, as a heartbeat's would be.\n                    Some(1)",
   "                    // at the next step, as a heartbeat's would be.\n                    None"),
 'limit_grower_no_rouse': (B+'purge.rs',
   "            // The thread is behind: what this step left owed is its to pay.\n            thread.rouse_any(self.space.os());",
   "            // The thread is behind: what this step left owed is its to pay."),
 'wait_on_reloaded_word': ('crates/asmalloc/src/depot/thread.rs',
   "self.os().wait(word.futex(), asleep.0, Some(every_ns));",
   "{\n                        let _ = asleep;\n                        self.os().wait(word.futex(), word.load().0, Some(every_ns));\n                    }"),
 'no_end_sleep': ('crates/asmalloc/src/depot/thread.rs',
   "                    self.end_sleep(key);\n", ""),
 'round_not_checked': ('crates/asmalloc/src/depot/thread.rs',
   "if !counted.round_done(self.os()) || self.source.thread_due(&now) {",
   "if self.source.thread_due(&now) {"),
 'thread_steps_no_plan': ('crates/asmalloc/src/depot/thread.rs',
   "        self.source.plan_thread();\n        returned", "        returned"),
 'no_step_between_round_steps': ('crates/asmalloc/src/depot/thread.rs',
   "            *returned |= self.thread_step().returned;\n", ""),
 'no_yield_after_step': ('crates/asmalloc/src/depot/thread.rs',
   "        let step = self.source.thread_step(&|| counted.now(self.os()));\n        self.os().yield_cpu();\n",
   "        let step = self.source.thread_step(&|| counted.now(self.os()));\n"),
 'no_fork_reset': ('crates/asmalloc/src/depot/generation.rs',
   "            && drawn\n", "            && drawn\n            && false\n"),
 'hold_stamp_once': (B+'hold.rs',
   "                Err(now) => seen = now,\n            }\n        }\n    }\n\n    /// Sets the thread's next look a hold after",
   "                Err(_) => return,\n            }\n        }\n    }\n\n    /// Sets the thread's next look a hold after"),
 'no_overdue_pass': (B+'purge.rs',
   "            let _ = self.decay_step(now, StepSize::HEARTBEAT);\n            started = true;",
   "            started = false;"),
 'thread_look_is_a_heartbeats': (B+'purge.rs',
   "            self.hold_maintain_step(&|| time)\n", "            self.hold_beat(time)\n"),
}
cmd = ['scripts/heavy', 'cargo', 'test', '-p', 'asmalloc', '--lib', '--quiet']
if os.environ.get('MUTATE_TESTS'):
    cmd = ['scripts/heavy', 'cargo', 'test', '-p', 'asmalloc', '--quiet', '--test', os.environ['MUTATE_TESTS']]
for name, (path, old, new) in M.items():
    if only and name not in only:
        continue
    full = os.path.join(wt, path)
    src = open(full).read()
    if src.count(old) != 1:
        print(f'{name}: PATTERN COUNT {src.count(old)}', flush=True)
        continue
    open(full, 'w').write(src.replace(old, new))
    try:
        r = subprocess.run(cmd, cwd=wt, capture_output=True, text=True)
        out = r.stdout + r.stderr
        fails = sorted({l.strip() for l in out.splitlines()
                        if l.startswith('    ') and '::' in l and ' ' not in l.strip()})
        fails = [f for f in fails if 'purge_off_path_review' not in f]
        err = [l for l in out.splitlines() if l.startswith('error')][:3]
        res = [l for l in out.splitlines() if l.startswith('test result')]
        extra = [l for l in out.splitlines() if 'FAILED' in l or 'failed:' in l][:4]
        print(f'{name}: caught by {len(fails)}: {fails[:6]} {res[-1:] if res else err} {extra if not fails else ""}', flush=True)
    finally:
        open(full, 'w').write(src)
