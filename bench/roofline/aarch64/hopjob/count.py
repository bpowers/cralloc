#!/usr/bin/env python3
"""Adds counters to a scratch copy of the repository, to see what a job asks of the slow paths:
count.py COPY. Each class gets its exchanges, its laundered magazines and objects, how the objects
of a laundered magazine lie (neighbours in the magazine one slot up or down from each other, and
whole magazines that are one run), and its fills. The workload prints them at exit. Kept apart from
variant.py, since it changes the routines' callers and no timing from it means anything.
"""
import os
import shutil
import sys

copy = sys.argv[1]
crate = os.path.join(copy, "crates/cralloc/src")


def patch(path, pairs):
    if not os.path.exists(path + ".orig"):
        shutil.copy(path, path + ".orig")
    s = open(path + ".orig").read()
    for old, new in pairs:
        assert s.count(old) == 1, old
        s = s.replace(old, new)
    open(path, "w").write(s)


patch(os.path.join(crate, "depot/mod.rs"), [
    ("""    fn launder_held(&self, class: SmallClass, magazine: &Magazine<'_>) -> usize {
        let level = magazine.level();
        if let Some(objects) = magazine.objects().get(..level) {
""", """    fn launder_held(&self, class: SmallClass, magazine: &Magazine<'_>) -> usize {
        let level = magazine.level();
        if let Some(objects) = magazine.objects().get(..level) {
            count_launder(class, objects);
"""),
    ("""/// """ + """Sends a magazine this thread holds to the depot for the domain the thread is in.""",
     """/// """ + """Sends a magazine this thread holds to the depot for the domain the thread is in."""),
])
s = open(os.path.join(crate, "depot/mod.rs")).read()
s += '''
/// Per class: alloc_slow, free_slow, laundered magazines, laundered objects, neighbours one slot up,
/// neighbours one slot down, magazines that are one run, fills, filled objects.
#[unsafe(no_mangle)]
pub static CRALLOC_COUNTS: [[core::sync::atomic::AtomicU64; 9]; 64] =
    [const { [const { core::sync::atomic::AtomicU64::new(0) }; 9] }; 64];

pub(crate) fn count(class: SmallClass, what: usize, by: u64) {
    if let Some(row) = CRALLOC_COUNTS.get(class.ordinal())
        && let Some(cell) = row.get(what)
    {
        cell.fetch_add(by, core::sync::atomic::Ordering::Relaxed);
    }
}

fn count_launder(class: SmallClass, objects: &[core::sync::atomic::AtomicPtr<u8>]) {
    use core::sync::atomic::Ordering::Relaxed;
    count(class, 2, 1);
    count(class, 3, objects.len() as u64);
    let size = class.size() as isize;
    let (mut up, mut down) = (0, 0);
    let mut last: Option<isize> = None;
    for object in objects {
        let at = object.load(Relaxed).addr() as isize;
        if let Some(before) = last {
            if at.wrapping_sub(before) == size {
                up += 1;
            } else if before.wrapping_sub(at) == size {
                down += 1;
            }
        }
        last = Some(at);
    }
    count(class, 4, up);
    count(class, 5, down);
    let pairs = objects.len().saturating_sub(1) as u64;
    if up == pairs || down == pairs {
        count(class, 6, 1);
    }
}
'''
open(os.path.join(crate, "depot/mod.rs"), "w").write(s)

patch(os.path.join(crate, "depot/slow.rs"), [
    ("""    pub fn alloc_slow(&self, class: SmallClass) -> Option<NonNull<u8>> {
        self.begin_slow_path();
""", """    pub fn alloc_slow(&self, class: SmallClass) -> Option<NonNull<u8>> {
        super::count(class, 0, 1);
        self.begin_slow_path();
"""),
    ("""    pub fn free_slow(&self, class: SmallClass, object: NonNull<u8>) {
        self.begin_slow_path();
""", """    pub fn free_slow(&self, class: SmallClass, object: NonNull<u8>) {
        super::count(class, 1, 1);
        self.begin_slow_path();
"""),
    ("""        let filled = self.source.fill(class, slots);
        let level = filled.count.min(capacity);
""", """        let filled = self.source.fill(class, slots);
        let level = filled.count.min(capacity);
        super::count(class, 7, 1);
        super::count(class, 8, level as u64);
"""),
])

path = os.path.join(copy, "bench/workloads/src/main.rs")
patch(path, [
    ("""fn main() {
""", """unsafe extern "C" {
    static CRALLOC_COUNTS: [[std::sync::atomic::AtomicU64; 9]; 64];
}

fn print_counts() {
    use std::sync::atomic::Ordering::Relaxed;
    eprintln!("class  alloc_slow  free_slow  laundered objects  up  down  one-run  fills filled");
    // SAFETY: a static of atomics.
    for (class, row) in unsafe { CRALLOC_COUNTS.iter().enumerate() } {
        let v: Vec<u64> = row.iter().map(|c| c.load(Relaxed)).collect();
        if v.iter().any(|&x| x != 0) {
            eprintln!("COUNT {class} {v:?}");
        }
    }
}

fn main() {
    real_main();
    print_counts();
}

fn real_main() {
"""),
])
