#!/usr/bin/env python3
"""A run-time switch for the refresh rule of `refresh-evidence` in a scratch copy of the tree
(docs/research/trough-gap.md, "Refresh on evidence"), so that the rule and the constant it
replaces are compared inside one binary: separate builds of a server move by more than the
difference with where their code lands (docs/research/grpc-placement.md). Probe only, never
product code:

    python3 -I refresh_switch.py TREE

With `TG_REFRESH=off` in the environment the back end reports no raise, so the refresh stays at
one retiring magazine in 32, the rule as it was; `TG_REFRESH=4` reports the most, one in 4
always (the constant `r4`); anything else, or unset, is the rule. The environment is read once,
from the `envp` the C library passes to `.init_array` functions, before `main`.
"""
import sys
from pathlib import Path

TREE = Path(sys.argv[1])
p = TREE / "crates/asmalloc/src/back/purge.rs"
s = p.read_text()
old = "    pub fn refresh_raise(&self) -> u32 {\n"
if s.count(old) != 1:
    sys.exit("refresh_switch: refresh_raise not found once")
s = s.replace(old, old + """        match TG_REFRESH.load(Relaxed) {
            1 => return 0,
            2 => return super::policy::REFRESH_RAISE_MAX,
            _ => {}
        }
""")
s += r'''

/// Scratch (refresh_switch.py): 0 the rule, 1 off, 2 always the most.
static TG_REFRESH: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

#[used]
#[unsafe(link_section = ".init_array")]
static TG_REFRESH_INIT: unsafe extern "C" fn(i32, *const *const u8, *const *const u8) = tg_refresh_init;

/// Reads `TG_REFRESH` from the environment the C library passes to `.init_array` functions.
unsafe extern "C" fn tg_refresh_init(_: i32, _: *const *const u8, envp: *const *const u8) {
    if envp.is_null() {
        return;
    }
    let mut at = envp;
    // SAFETY: scratch probe; the C library passes a NULL-terminated array of NUL-terminated
    // strings.
    unsafe {
        while !(*at).is_null() {
            let e = *at;
            let name = b"TG_REFRESH=";
            let mut i = 0;
            while i < name.len() && *e.add(i) == name[i] {
                i += 1;
            }
            if i == name.len() {
                let (a, b) = (*e.add(i), *e.add(i + 1));
                let v = if a == b'o' && b == b'f' { 1 } else if a == b'4' { 2 } else { 0 };
                TG_REFRESH.store(v, Relaxed);
                return;
            }
            at = at.add(1);
        }
    }
}
'''
p.write_text(s)
print("refresh_switch: applied")
