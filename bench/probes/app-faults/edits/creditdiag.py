import os
import sys
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from sub import sub

# Diagnostic counters for the credit diagnostic (`docs/research/hold-credit.md`), applied after
# `credit2.py` or to main alone. At exit (`.fini_array`) it writes, to stderr, one line
# "CREDITDIAG key value ..." of totals, and to SERIES_DIR/<pid>.series a line every 2 ms or more
# (at a heartbeat) with the process's anonymous memory and the allocator's state, for finding
# what was owed and retained when the peak was reached:
#
#   t_ms anon_mib debt_mib ret_small_mib ret_large_mib carve_small carve_large refault_large
#   refault_small paid1 paid2 hold decay dropped cover     (all after the first four: MiB so far)
#
# Totals: bytes carved (small, large), bytes faulted back under claims (large, small: the pages
# a claim found returned), bytes a large block's growth in place found returned or unwritten
# (`cover`), growth payments (count, dry ones, owed, paid by each pass, words read,
# cycles), debt dropped (taken but not owed, being under the cap or under a step), bytes the hold
# and the clocked pass returned, and the background thread's start and rounds.
series_dir = os.environ.get("SERIES_DIR", "/tmp")

open("crates/asmalloc/src/os/linux/creditdiag.rs", "w").write(r'''
//! DIAGNOSTIC ONLY, never committed: the credit diagnostic's counters and series.
use super::syscall::{int, syscall};
use core::fmt::Write;
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering::Relaxed};

macro_rules! counters {
    ($($name:ident),*) => { $(pub(crate) static $name: AtomicU64 = AtomicU64::new(0);)* };
}
counters!(
    CARVE_SMALL, CARVE_LARGE, REFAULT_LARGE, REFAULT_SMALL, PAYMENTS, DRY, OWED, PAID1, PAID2,
    WORDS, TICKS, MAX_TICKS, DROPPED, HOLD, DECAY, THREAD_START_MS, ROUNDS, WORK_ROUNDS,
    CLAIMS_LARGE, CLAIMS_LARGE_REFAULT, AVOIDABLE, AVOIDABLE_WHOLE, SCANS, SCAN_SLOTS,
    SMALL_FILL_REFAULT, SMALL_AVOIDABLE, LAST_TICK, COVER
);

#[inline(always)]
pub(crate) fn add(c: &AtomicU64, v: usize) {
    if v != 0 {
        c.fetch_add(v as u64, Relaxed);
    }
}

pub(crate) fn tsc() -> u64 {
    // SAFETY: rdtsc has no preconditions.
    unsafe { core::arch::x86_64::_rdtsc() }
}

pub(crate) fn now_ms() -> u64 {
    let mut ts = [0u64; 2];
    // SAFETY: clock_gettime(CLOCK_MONOTONIC) into a live local buffer.
    unsafe { syscall(228, [int(1), ts.as_mut_ptr().cast(), int(0), int(0), int(0), int(0)]) };
    ts[0] * 1000 + ts[1] / 1_000_000
}

pub(crate) fn pay(owed: usize, debt: usize, first: usize, purged: usize, words: usize, ticks: u64) {
    add(&DROPPED, debt.saturating_sub(owed));
    if owed == 0 {
        return;
    }
    PAYMENTS.fetch_add(1, Relaxed);
    DRY.fetch_add(u64::from(purged == 0), Relaxed);
    add(&OWED, owed);
    add(&PAID1, first);
    add(&PAID2, purged - first);
    add(&WORDS, words);
    TICKS.fetch_add(ticks, Relaxed);
    MAX_TICKS.fetch_max(ticks, Relaxed);
}

const FIELDS: usize = 15;
const CAP: usize = 1 << 16;
static mut SERIES: [[u64; FIELDS]; CAP] = [[0; FIELDS]; CAP];
static NEXT: AtomicUsize = AtomicUsize::new(0);

fn anon_pages() -> u64 {
    let mut buf = [0u8; 128];
    let path = b"/proc/self/statm\0";
    // SAFETY: openat, read and close on a live local path and buffer.
    let n = unsafe {
        let fd = syscall(257, [int(-100isize as usize), path.as_ptr() as *mut u8, int(0), int(0), int(0), int(0)]);
        let fd = fd.addr();
        if fd > 4096 * 1024 * 1024 {
            return 0;
        }
        let n = syscall(0, [int(fd), buf.as_mut_ptr(), int(buf.len()), int(0), int(0), int(0)]).addr();
        syscall(3, [int(fd), int(0), int(0), int(0), int(0), int(0)]);
        n
    };
    if n > buf.len() {
        return 0;
    }
    let mut fields = [0u64; 3];
    let mut i = 0;
    for &c in &buf[..n] {
        if c == b' ' || c == b'\n' {
            i += 1;
            if i == 3 {
                break;
            }
        } else if c.is_ascii_digit() {
            fields[i] = fields[i] * 10 + u64::from(c - b'0');
        }
    }
    fields[1].saturating_sub(fields[2])
}

/// At a heartbeat: a sample, if 2^23 cycles (about 2 ms) have passed since the last.
pub(crate) fn sample(debt: usize, small: usize, large: usize) {
    let t = tsc();
    let last = LAST_TICK.load(Relaxed);
    if t.wrapping_sub(last) < 1 << 23 || LAST_TICK.compare_exchange(last, t, Relaxed, Relaxed).is_err() {
        return;
    }
    let i = NEXT.fetch_add(1, Relaxed);
    if i >= CAP {
        return;
    }
    let row = [
        now_ms(),
        anon_pages() * 4096,
        debt as u64,
        small as u64,
        large as u64,
        CARVE_SMALL.load(Relaxed),
        CARVE_LARGE.load(Relaxed),
        REFAULT_LARGE.load(Relaxed),
        REFAULT_SMALL.load(Relaxed),
        PAID1.load(Relaxed),
        PAID2.load(Relaxed),
        HOLD.load(Relaxed),
        DECAY.load(Relaxed),
        DROPPED.load(Relaxed),
        COVER.load(Relaxed),
    ];
    // SAFETY: row i was handed to this caller alone by the fetch_add.
    unsafe { (&raw mut SERIES).cast::<[u64; FIELDS]>().add(i).write(row) };
}

struct Buf {
    b: [u8; 4096],
    n: usize,
    fd: usize,
}

impl Buf {
    fn flush(&mut self) {
        // SAFETY: write(2) from a live local buffer.
        unsafe { syscall(1, [int(self.fd), self.b.as_mut_ptr(), int(self.n), int(0), int(0), int(0)]) };
        self.n = 0;
    }
}

impl Write for Buf {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        if self.n + s.len() > self.b.len() {
            self.flush();
        }
        for &c in s.as_bytes() {
            if self.n < self.b.len() {
                self.b[self.n] = c;
                self.n += 1;
            }
        }
        Ok(())
    }
}

extern "C" fn dump() {
    let mut b = Buf { b: [0; 4096], n: 0, fd: 2 };
    let _ = write!(b, "CREDITDIAG");
    for (k, c) in [
        ("carve_small", &CARVE_SMALL), ("carve_large", &CARVE_LARGE),
        ("refault_large", &REFAULT_LARGE), ("refault_small", &REFAULT_SMALL),
        ("payments", &PAYMENTS), ("dry", &DRY), ("owed", &OWED), ("paid1", &PAID1),
        ("paid2", &PAID2), ("words", &WORDS), ("ticks", &TICKS), ("max_ticks", &MAX_TICKS),
        ("dropped", &DROPPED), ("hold", &HOLD), ("decay", &DECAY),
        ("thread_start_ms", &THREAD_START_MS), ("rounds", &ROUNDS), ("work_rounds", &WORK_ROUNDS),
        ("claims_large", &CLAIMS_LARGE), ("claims_large_refault", &CLAIMS_LARGE_REFAULT),
        ("avoidable", &AVOIDABLE), ("avoidable_whole", &AVOIDABLE_WHOLE), ("scans", &SCANS),
        ("scan_slots", &SCAN_SLOTS), ("small_fill_refault", &SMALL_FILL_REFAULT),
        ("small_avoidable", &SMALL_AVOIDABLE), ("cover", &COVER),
    ] {
        let _ = write!(b, " {k} {}", c.load(Relaxed));
    }
    let _ = writeln!(b, " start_ms {} end_ms {}", START_MS.load(Relaxed), now_ms());
    b.flush();
    let mut path = Buf { b: [0; 4096], n: 0, fd: 0 };
    // SAFETY: getpid has no preconditions.
    let pid = unsafe { syscall(39, [int(0); 6]) }.addr();
    let _ = write!(path, "SERIES_DIR/{pid}.series\0");
    // SAFETY: openat on a live local path: O_WRONLY | O_CREAT | O_TRUNC, 0644.
    let fd = unsafe {
        syscall(257, [int(-100isize as usize), path.b.as_mut_ptr(), int(0o1101), int(0o644), int(0), int(0)])
    }
    .addr();
    if fd > 1 << 20 {
        return;
    }
    let mut out = Buf { b: [0; 4096], n: 0, fd };
    let rows = NEXT.load(Relaxed).min(CAP);
    for i in 0..rows {
        // SAFETY: rows below NEXT were written before the exit that runs this.
        let r = unsafe { (&raw const SERIES).cast::<[u64; FIELDS]>().add(i).read() };
        let _ = write!(out, "{}", r[0].saturating_sub(START_MS.load(Relaxed)));
        for v in &r[1..] {
            let _ = write!(out, " {:.1}", *v as f64 / 1048576.0);
        }
        let _ = writeln!(out);
    }
    out.flush();
    // SAFETY: close on the descriptor opened above.
    unsafe { syscall(3, [int(fd), int(0), int(0), int(0), int(0), int(0)]) };
}

pub(crate) static START_MS: AtomicU64 = AtomicU64::new(0);

extern "C" fn start() {
    START_MS.store(now_ms(), Relaxed);
}

#[used]
#[unsafe(link_section = ".init_array")]
static INIT: extern "C" fn() = start;

#[used]
#[unsafe(link_section = ".fini_array")]
static FINI: extern "C" fn() = dump;
'''.replace("SERIES_DIR", series_dir))
sub("crates/asmalloc/src/os/linux/mod.rs", "mod syscall;\n", "mod syscall;\npub(crate) mod creditdiag;\n")
D = "crate::os::linux::creditdiag"

R = "crates/asmalloc/src/back/region.rs"
# Small claims: every page a claim's runs found returned faults back when written.
sub(R, """    let claimed = bits.count_ones() as usize * layout.size();
    Settled {""", f"""    {D}::add(&{D}::REFAULT_SMALL, cleaned * page);
    let claimed = bits.count_ones() as usize * layout.size();
    Settled {{""")
# Growth in place: the pages under the block's new extent that were clean.
sub(R, """        if word.load() & inside != 0 {
""", f"""        if word.load() & inside != 0 {{
            {D}::add(&{D}::COVER, count(word.load() & inside) * layout.page_size());
""")
# Large claims: the returned pages under the block.
sub(R, """        let seen = word.load() & mask;
        clean += count(seen);
""", f"""        let seen = word.load() & mask;
        clean += count(seen);
        {D}::add(&{D}::REFAULT_LARGE, count(seen & mask & below(w, end)) * layout.page_size());
""")

C = "crates/asmalloc/src/back/class.rs"
sub(C, """        self.grow(carved);
        got""", f"""        {D}::add(&{D}::CARVE_SMALL, carved);
        self.grow(carved);
        got""")
sub(C, """            self.grow(if small { bytes } else { size });""", f"""            {D}::add(if small {{ &{D}::CARVE_SMALL }} else {{ &{D}::CARVE_LARGE }}, if small {{ bytes }} else {{ size }});
            self.grow(if small {{ bytes }} else {{ size }});""")
sub("crates/asmalloc/src/back/run.rs", """            self.grow(size);
            // A run""", f"""            {D}::add(&{D}::CARVE_LARGE, size);
            self.grow(size);
            // A run""")

P = "crates/asmalloc/src/back/purge.rs"
sub(P, """        let debt = self.state.growth.swap(0, Relaxed);
""", f"""        let t0 = {D}::tsc();
        let debt = self.state.growth.swap(0, Relaxed);
""")
s = open(P).read()
if "// CREDIT2 PAY END" in s:
    sub(P, "        // CREDIT2 PAY END\n", f"""        {D}::pay(owed, debt, first, purged, spent + spent2, {D}::tsc() - t0);
""")
    sub(P, "        let _ = first;\n", "")
else:
    sub(P, """        if owed > goal && purged != 0 {
            self.state.growth.fetch_add(owed - goal, Relaxed);
        }
""", f"""        if owed > goal && purged != 0 {{
            self.state.growth.fetch_add(owed - goal, Relaxed);
        }}
        {D}::pay(owed, debt, purged, purged, spent, {D}::tsc() - t0);
""")
sub(P, """            *returned += purged;
""", f"""            *returned += purged;
            {D}::add(&{D}::DECAY, purged);
""")
sub(P, """        {
            self.pay_growth(step);
        }
    }
""", f"""        {{
            self.pay_growth(step);
        }}
        {D}::sample(
            self.state.growth.load(Relaxed),
            self.clamped(false),
            self.clamped(true),
        );
    }}
""")

H = "crates/asmalloc/src/back/hold.rs"
sub(H, """        PassStep {
            more: stopped && purged != 0 && purged < over,""", f"""        {D}::add(&{D}::HOLD, purged);
        PassStep {{
            more: stopped && purged != 0 && purged < over,""")

sub("crates/asmalloc/src/heap/background.rs", """extern "C" fn run(_: *mut c_void) -> *mut c_void {
""", f"""extern "C" fn run(_: *mut c_void) -> *mut c_void {{
    {D}::THREAD_START_MS.store({D}::now_ms(), core::sync::atomic::Ordering::Relaxed);
""")
sub("crates/asmalloc/src/depot/thread.rs", """        let Some(work) = round else {
            return Rest::Sleep;
        };
""", f"""        let Some(work) = round else {{
            return Rest::Sleep;
        }};
        {D}::add(&{D}::ROUNDS, 1);
        {D}::add(&{D}::WORK_ROUNDS, usize::from(work));
""")
