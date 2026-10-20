//! Where asmalloc's resident memory is, sampled while a workload runs (feature
//! `asmalloc-memstat`; method and results in docs/research/memory.md).
//!
//! With `ASMALLOC_MEMSTAT=<file>` set, a thread started before `main` wakes every
//! `ASMALLOC_MEMSTAT_MS` milliseconds (default 10), walks the heap with `Asmalloc::inspect`,
//! asks the kernel which pages of every range the walk names are resident (`mincore`), and
//! attributes each resident byte to one use:
//!
//! - `live`: slots the program holds (carved, not free in a bitmap, not in a magazine);
//! - `loaded`, `spare`, `depot`: objects parked in magazines, by where the magazine is;
//! - `free`: free in the back end's bitmap, on a page that is still resident;
//! - `above`: resident above a region's watermark (the tail of the page being carved, or the
//!   rest of a huge page);
//! - `runs`, `meta` (region metadata blocks), `mags` (the magazine area), `slabs`.
//!
//! It writes one line per sample, and at exit the full breakdown by class and by CPU of the
//! sample in which the program held the most (`peak`) and of one last sample (`final`).
//!
//! The sampler never calls the global allocator, so it does not disturb what it measures: its
//! thread comes from `pthread_create` (glibc's own heap and `mmap`), its buffers from `mmap`,
//! and it formats into a fixed buffer. The walk is a racy snapshot, so a sample taken while
//! threads allocate can count an object twice or miss it; sums are exact only when the heap is
//! quiet. It pins itself to `ASMALLOC_MEMSTAT_CPU` (default 14).
use asmalloc::classes::{Class, MagazineSize, SmallClass};
use asmalloc::stats::{Place, Region, Visitor};
use std::fmt::Write as _;
use std::sync::atomic::{AtomicBool, Ordering::*};

const PAGE: usize = 4096;
const CLASSES: usize = asmalloc::classes::COUNT + 1;
const CPUS: usize = 64;
const MAX_REGIONS: usize = 4096;
/// One byte per page of every region: enough for 128 GiB of committed regions.
const SCRATCH: usize = 32 << 20;
/// One byte per page for ranges looked at once: 4 GiB at a time.
const TEMP: usize = 1 << 20;

#[derive(Clone, Copy, Default)]
struct ClassStat {
    regions: u32,
    carved: u64,
    committed: u64,
    huge_part: u64,
    resident: u64,
    above: u64,
    free: u64,
    free_res: u64,
    /// Resident bytes of pages that lie wholly inside one run of free slots: purgeable.
    free_whole: u64,
    cached: [u64; 3],
    cached_res: [u64; 3],
    magazines: [u32; 3],
    meta_res: u64,
}

#[derive(Clone, Copy)]
struct Snap {
    t_ms: u64,
    rss: u64,
    /// AnonHugePages of the process, when `ASMALLOC_MEMSTAT_THP` is set.
    thp: u64,
    /// Bytes the program asked for and holds, by the class of the layout it asked with.
    req: [i64; CLASSES],
    /// The same, by the class of the slot that holds them (the last entry: not yet known).
    req_slot: [i64; CLASSES],
    retained: u64,
    classes: [ClassStat; CLASSES],
    cpu_res: [[u64; 2]; CPUS],
    cpus: u32,
    runs_res: u64,
    meta_res: u64,
    mags_res: u64,
    mags_carved: u64,
    slabs_res: u64,
    empties: u32,
    /// Where things are, for a reader that attributes pages by address (acct.py).
    regions: [RegionAt; MAX_LISTED],
    listed: usize,
    mags_base: u64,
    mags_committed: u64,
    slabs_base: u64,
}

/// The regions listed in a dump.
const MAX_LISTED: usize = 256;

#[derive(Clone, Copy)]
struct RegionAt {
    index: u32,
    class: u8,
    base: u64,
    meta: u64,
    meta_len: u64,
    len: u64,
}

impl Snap {
    const ZERO: Snap = Snap {
        t_ms: 0,
        rss: 0,
        thp: 0,
        req: [0; CLASSES],
        req_slot: [0; CLASSES],
        retained: 0,
        classes: [ClassStat {
            regions: 0,
            carved: 0,
            committed: 0,
            huge_part: 0,
            resident: 0,
            above: 0,
            free: 0,
            free_res: 0,
            free_whole: 0,
            cached: [0; 3],
            cached_res: [0; 3],
            magazines: [0; 3],
            meta_res: 0,
        }; CLASSES],
        cpu_res: [[0; 2]; CPUS],
        cpus: 0,
        runs_res: 0,
        meta_res: 0,
        mags_res: 0,
        mags_carved: 0,
        slabs_res: 0,
        empties: 0,
        regions: [RegionAt { index: 0, class: 0, base: 0, meta: 0, meta_len: 0, len: 0 }; MAX_LISTED],
        listed: 0,
        mags_base: 0,
        mags_committed: 0,
        slabs_base: 0,
    };

    /// Resident bytes by use: live, loaded, spare, depot, free, above.
    fn uses(&self) -> [u64; 6] {
        let mut u = [0u64; 6];
        for c in &self.classes {
            let cached: u64 = c.cached_res.iter().sum();
            u[0] += c.resident.saturating_sub(c.above + c.free_res + cached);
            u[1] += c.cached_res[0];
            u[2] += c.cached_res[1];
            u[3] += c.cached_res[2];
            u[4] += c.free_res;
            u[5] += c.above;
        }
        u
    }

    fn heap(&self) -> u64 {
        let regions: u64 = self.classes.iter().map(|c| c.resident).sum();
        regions + self.runs_res + self.meta_res + self.mags_res + self.slabs_res
    }
}

/// The residency of the regions seen in this walk: where each region's bytes are in the
/// scratch buffer.
#[derive(Clone, Copy)]
struct Rec {
    base: usize,
    pages: usize,
    at: usize,
}

struct Walk {
    snap: Snap,
    scratch: *mut u8,
    temp: *mut u8,
    used: usize,
    recs: [Rec; 512],
    count: usize,
    /// Region number to record number plus one.
    by_index: [u16; MAX_REGIONS],
    heap_base: usize,
    region_shift: u32,
    current: usize,
}

/// Fills `out` with one byte per page of `[start, start + len)`, non-zero where resident.
/// `start` is page-aligned. Pages the kernel cannot answer for read as not resident.
fn residency(start: usize, len: usize, out: *mut u8) {
    // SAFETY: mincore only reads the page tables; `out` has a byte per page, as every caller
    // sizes it.
    let rc = unsafe { libc::mincore(start as *mut libc::c_void, len, out) };
    if rc != 0 {
        // SAFETY: as above.
        unsafe { out.write_bytes(0, len.div_ceil(PAGE)) };
    }
}

impl Walk {
    /// Resident bytes of `[start, start + len)` through `temp`, a range looked at once.
    fn resident_once(&mut self, start: usize, len: usize) -> u64 {
        let (mut at, end) = (start & !(PAGE - 1), (start + len).next_multiple_of(PAGE));
        let mut total = 0u64;
        while at < end {
            let chunk = (end - at).min(TEMP * PAGE);
            residency(at, chunk, self.temp);
            // SAFETY: `temp` is TEMP bytes and the chunk has at most that many pages.
            let bytes = unsafe { std::slice::from_raw_parts(self.temp, chunk / PAGE) };
            total += bytes.iter().filter(|&&b| b & 1 != 0).count() as u64 * PAGE as u64;
            at += chunk;
        }
        total
    }

    /// Resident bytes of `[start, start + len)` inside region record `rec`.
    fn overlap(&self, rec: usize, start: usize, len: usize) -> u64 {
        let r = self.recs[rec];
        if start < r.base {
            return 0;
        }
        let (mut at, end) = (start - r.base, (start - r.base + len).min(r.pages * PAGE));
        let mut total = 0;
        while at < end {
            let page = at / PAGE;
            let stop = ((page + 1) * PAGE).min(end);
            // SAFETY: `page < r.pages`, and the record's bytes lie inside the scratch buffer.
            if unsafe { self.scratch.add(r.at + page).read() } & 1 != 0 {
                total += (stop - at) as u64;
            }
            at = stop;
        }
        total
    }

    fn record_of(&self, address: usize) -> Option<usize> {
        let index = address.checked_sub(self.heap_base)? >> self.region_shift;
        match *self.by_index.get(index)? {
            0 => None,
            n => Some(n as usize - 1),
        }
    }
}

impl Visitor for Walk {
    fn retained(&mut self, bytes: usize) {
        self.snap.retained = bytes as u64;
    }

    fn region(&mut self, region: &Region) {
        let c = &mut self.snap.classes[region.class.index() as usize];
        c.regions += 1;
        // What is carved and committed, not the address space below the class's first slot.
        c.carved += region.carved.saturating_sub(region.origin) as u64;
        c.committed += region.committed.saturating_sub(region.origin) as u64;
        if let Some(from) = region.huge_from {
            c.huge_part += region.committed.saturating_sub(from) as u64;
        }
        let pages = region.committed / PAGE;
        if self.count == self.recs.len() || self.used + pages > SCRATCH || region.index >= MAX_REGIONS {
            self.current = usize::MAX;
            return;
        }
        let base = region.base.addr();
        // SAFETY: the record's bytes are inside the scratch buffer, checked just above.
        residency(base, pages * PAGE, unsafe { self.scratch.add(self.used) });
        self.recs[self.count] = Rec { base, pages, at: self.used };
        self.by_index[region.index] = self.count as u16 + 1;
        self.heap_base = base - region.index * region.len;
        self.region_shift = region.len.trailing_zeros();
        HEAP_BASE.store(self.heap_base, Relaxed);
        REGION_SHIFT.store(self.region_shift as usize, Relaxed);
        REGION_CLASS[region.index].store(region.class.index() + 1, Relaxed);
        self.current = self.count;
        self.count += 1;
        self.used += pages;
        let resident = self.overlap(self.current, base, pages * PAGE);
        let above = self.overlap(self.current, base + region.carved, pages * PAGE - region.carved.min(pages * PAGE));
        let c = &mut self.snap.classes[region.class.index() as usize];
        c.resident += resident;
        c.above += above;
        let meta = self.resident_once(region.metadata.addr(), region.metadata_len);
        self.snap.meta_res += meta;
        self.snap.classes[region.class.index() as usize].meta_res += meta;
        if self.snap.listed < MAX_LISTED {
            self.snap.regions[self.snap.listed] = RegionAt {
                index: region.index as u32,
                class: region.class.index(),
                base: base as u64,
                meta: region.metadata.addr() as u64,
                meta_len: region.metadata_len as u64,
                len: region.len as u64,
            };
            self.snap.listed += 1;
        }
    }

    fn free(&mut self, class: Class, start: *const u8, len: usize) {
        let resident = if self.current == usize::MAX { 0 } else { self.overlap(self.current, start.addr(), len) };
        let (first, end) = (start.addr().next_multiple_of(PAGE), (start.addr() + len) & !(PAGE - 1));
        let whole =
            if self.current == usize::MAX || first >= end { 0 } else { self.overlap(self.current, first, end - first) };
        let c = &mut self.snap.classes[class.index() as usize];
        c.free += len as u64;
        c.free_res += resident;
        c.free_whole += whole;
    }

    fn run(&mut self, base: *const u8, len: usize, _size: usize) {
        self.snap.runs_res += self.resident_once(base.addr(), len);
    }

    fn magazine_area(&mut self, base: *const u8, carved: usize, committed: usize) {
        self.snap.mags_carved = carved as u64;
        self.snap.mags_base = base.addr() as u64;
        self.snap.mags_committed = committed as u64;
        self.snap.mags_res = self.resident_once(base.addr(), committed);
    }

    fn slabs(&mut self, base: *const u8, bytes: usize, count: usize) {
        self.snap.cpus = count as u32;
        self.snap.slabs_base = base.addr() as u64;
        self.snap.slabs_res = self.resident_once(base.addr(), bytes * count);
    }

    fn magazine(&mut self, place: Place, class: SmallClass, _objects: usize) {
        self.snap.classes[class.class().index() as usize].magazines[kind(place).0] += 1;
    }

    fn cached(&mut self, place: Place, class: SmallClass, object: *const u8) {
        let size = class.size();
        let resident = self.record_of(object.addr()).map_or(0, |rec| self.overlap(rec, object.addr(), size));
        let (k, cpu) = kind(place);
        let c = &mut self.snap.classes[class.class().index() as usize];
        c.cached[k] += size as u64;
        c.cached_res[k] += resident;
        if let Some(cpu) = cpu
            && (cpu as usize) < CPUS
        {
            self.snap.cpu_res[cpu as usize][k] += resident;
        }
    }

    fn empties(&mut self, _size: MagazineSize, count: usize) {
        self.snap.empties += count as u32;
    }
}

fn kind(place: Place) -> (usize, Option<u32>) {
    match place {
        Place::Loaded(cpu) => (0, Some(cpu)),
        Place::Spare(cpu) => (1, Some(cpu)),
        Place::Depot => (2, None),
    }
}

/// A fixed buffer that formats without allocating and writes to a file descriptor.
struct Out {
    fd: i32,
    len: usize,
    buf: [u8; 4096],
}

impl Out {
    fn flush(&mut self) {
        let mut done = 0;
        while done < self.len {
            // SAFETY: the buffer holds `len` initialized bytes.
            let n = unsafe { libc::write(self.fd, self.buf.as_ptr().add(done).cast(), self.len - done) };
            if n <= 0 {
                break;
            }
            done += n as usize;
        }
        self.len = 0;
    }
}

impl std::fmt::Write for Out {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        for chunk in s.as_bytes().chunks(self.buf.len()) {
            if self.len + chunk.len() > self.buf.len() {
                self.flush();
            }
            self.buf[self.len..self.len + chunk.len()].copy_from_slice(chunk);
            self.len += chunk.len();
        }
        Ok(())
    }
}

struct Sampler {
    walk: Walk,
    peak: Snap,
    /// The sample with the largest resident set.
    peak_rss: Snap,
    /// `/proc/self/smaps_rollup`, open when `ASMALLOC_MEMSTAT_THP` is set, else 0.
    rollup: i32,
    out: Out,
    statm: i32,
    start_ms: u64,
    interval_ms: u64,
}

static STOP: AtomicBool = AtomicBool::new(false);
/// The sampler's state, in memory from `mmap`. Written by the constructor before the thread
/// starts, then used only by the thread, then only by the exit handler after joining it.
static mut SAMPLER: *mut Sampler = std::ptr::null_mut();
static mut THREAD: libc::pthread_t = 0;

fn now_ms() -> u64 {
    let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: clock_gettime writes one timespec.
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
    ts.tv_sec as u64 * 1000 + ts.tv_nsec as u64 / 1_000_000
}

fn map(len: usize) -> *mut u8 {
    // SAFETY: a fresh private anonymous mapping.
    let p = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            len,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_PRIVATE | libc::MAP_ANONYMOUS | libc::MAP_NORESERVE,
            -1,
            0,
        )
    };
    if p == libc::MAP_FAILED { std::ptr::null_mut() } else { p.cast() }
}

impl Sampler {
    fn thp(&self) -> u64 {
        if self.rollup <= 0 {
            return 0;
        }
        let mut buf = [0u8; 2048];
        // SAFETY: pread writes at most the buffer's length.
        let n = unsafe { libc::pread(self.rollup, buf.as_mut_ptr().cast(), buf.len(), 0) };
        let text = std::str::from_utf8(&buf[..n.max(0) as usize]).unwrap_or("");
        text.lines()
            .find(|l| l.starts_with("AnonHugePages:"))
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|k| k.parse::<u64>().ok())
            .unwrap_or(0)
            * 1024
    }

    fn rss(&self) -> u64 {
        let mut buf = [0u8; 128];
        // SAFETY: pread writes at most the buffer's length.
        let n = unsafe { libc::pread(self.statm, buf.as_mut_ptr().cast(), buf.len(), 0) };
        let text = std::str::from_utf8(&buf[..n.max(0) as usize]).unwrap_or("");
        text.split(' ').nth(1).and_then(|s| s.parse::<u64>().ok()).unwrap_or(0) * PAGE as u64
    }

    fn sample(&mut self) {
        let w = &mut self.walk;
        w.snap = Snap::ZERO;
        w.used = 0;
        w.count = 0;
        w.by_index = [0; MAX_REGIONS];
        w.current = usize::MAX;
        asmalloc::Asmalloc::inspect(w);
        w.snap.t_ms = now_ms() - self.start_ms;
        self.walk.snap.rss = self.rss();
        self.walk.snap.thp = self.thp();
        for (i, r) in REQ.iter().enumerate() {
            self.walk.snap.req[i] = r.load(Relaxed);
        }
        for (i, r) in REQ_SLOT.iter().enumerate() {
            self.walk.snap.req_slot[i] = r.load(Relaxed);
        }
        if self.walk.snap.rss > self.peak_rss.rss {
            self.peak_rss = self.walk.snap;
        }
        let snap = &self.walk.snap;
        let u = snap.uses();
        let _ = writeln!(
            self.out,
            "sample probe_live={} t_ms={} rss={} heap={} live={} loaded={} spare={} depot={} free={} above={} runs={} meta={} mags={} slabs={} retained={}",
            probe_live(),
            snap.t_ms,
            snap.rss,
            snap.heap(),
            u[0],
            u[1],
            u[2],
            u[3],
            u[4],
            u[5],
            snap.runs_res,
            snap.meta_res,
            snap.mags_res,
            snap.slabs_res,
            snap.retained
        );
        if u[0] + snap.runs_res > self.peak.uses()[0] + self.peak.runs_res || self.peak.heap() == 0 {
            self.peak = *snap;
        }
    }

    fn dump(out: &mut Out, label: &str, snap: &Snap) {
        let u = snap.uses();
        let _ = writeln!(
            out,
            "{label} t_ms={} rss={} thp={} req={} heap={} live={} loaded={} spare={} depot={} free={} above={} runs={} meta={} mags={} mags_carved={} slabs={} cpus={} retained={} empty_magazines={}",
            snap.t_ms,
            snap.rss,
            snap.thp,
            snap.req.iter().sum::<i64>(),
            snap.heap(),
            u[0],
            u[1],
            u[2],
            u[3],
            u[4],
            u[5],
            snap.runs_res,
            snap.meta_res,
            snap.mags_res,
            snap.mags_carved,
            snap.slabs_res,
            snap.cpus,
            snap.retained,
            snap.empties
        );
        for (index, c) in snap.classes.iter().enumerate() {
            if c.regions == 0 && snap.req[index] == 0 {
                continue;
            }
            let size = Class::from_index(index as u8).map_or(0, Class::size);
            let cached: u64 = c.cached_res.iter().sum();
            let _ = writeln!(
                out,
                "{label}-class size={size} req={} req_slot={} free_whole={} regions={} carved={} committed={} huge_part={} resident={} live={} loaded={}/{} spare={}/{} depot={}/{} free={}/{} above={} magazines={}/{}/{} meta={}",
                snap.req[index],
                snap.req_slot[index],
                c.free_whole,
                c.regions,
                c.carved,
                c.committed,
                c.huge_part,
                c.resident,
                c.resident.saturating_sub(c.above + c.free_res + cached),
                c.cached_res[0],
                c.cached[0],
                c.cached_res[1],
                c.cached[1],
                c.cached_res[2],
                c.cached[2],
                c.free_res,
                c.free,
                c.above,
                c.magazines[0],
                c.magazines[1],
                c.magazines[2],
                c.meta_res
            );
        }
        let _ = writeln!(
            out,
            "{label}-where mags_base={:x} mags_committed={} slabs_base={:x}",
            snap.mags_base, snap.mags_committed, snap.slabs_base
        );
        for r in &snap.regions[..snap.listed] {
            let size = Class::from_index(r.class).map_or(0, Class::size);
            let _ = writeln!(
                out,
                "{label}-region index={} size={size} base={:x} len={} meta={:x} meta_len={}",
                r.index, r.base, r.len, r.meta, r.meta_len
            );
        }
        for (cpu, r) in snap.cpu_res.iter().enumerate() {
            if r[0] + r[1] > 0 {
                let _ = writeln!(out, "{label}-cpu cpu={cpu} loaded={} spare={}", r[0], r[1]);
            }
        }
        out.flush();
    }
}

extern "C" fn run(_: *mut libc::c_void) -> *mut libc::c_void {
    // SAFETY: the constructor finished writing SAMPLER before creating this thread, and until
    // the exit handler has joined it nothing else touches it.
    let sampler = unsafe { &mut *SAMPLER };
    let interval = libc::timespec {
        tv_sec: (sampler.interval_ms / 1000) as libc::time_t,
        tv_nsec: (sampler.interval_ms % 1000 * 1_000_000) as libc::c_long,
    };
    while !STOP.load(Acquire) {
        sampler.sample();
        sampler.out.flush();
        // SAFETY: nanosleep reads one timespec.
        unsafe { libc::nanosleep(&interval, std::ptr::null_mut()) };
    }
    std::ptr::null_mut()
}

extern "C" fn finish() {
    STOP.store(true, Release);
    // SAFETY: THREAD was written by the constructor, which registered this handler after.
    unsafe { libc::pthread_join(THREAD, std::ptr::null_mut()) };
    // SAFETY: the sampler thread has ended, so this handler is the state's only user.
    let sampler = unsafe { &mut *SAMPLER };
    sampler.sample();
    Sampler::dump(&mut sampler.out, "peak", &sampler.peak);
    for (op, row) in PROBE.iter().enumerate() {
        for (b, n) in row.iter().enumerate() {
            let n = n.load(Relaxed);
            if n != 0 {
                let _ = writeln!(sampler.out, "probe op={op} size_log2={b} count={n}");
            }
        }
    }
    let peak_rss = sampler.peak_rss;
    Sampler::dump(&mut sampler.out, "peakrss", &peak_rss);
    Sampler::dump(&mut sampler.out, "final", &sampler.walk.snap);
    // The sampler's own memory, which a whole-process account (memacct, acct.py) leaves out.
    // SAFETY: as above; this reads the pointer, not through it.
    let state = unsafe { SAMPLER }.addr();
    let (scratch, temp) = (sampler.walk.scratch.addr(), sampler.walk.temp.addr());
    for (start, len) in [(state, size_of::<Sampler>()), (scratch, SCRATCH), (temp, TEMP)] {
        let _ = writeln!(sampler.out, "sampler-mem {start:x} {len}");
    }
    sampler.out.flush();
}

fn env_num(name: &std::ffi::CStr, default: u64) -> u64 {
    // SAFETY: getenv returns null or a NUL-terminated string that outlives this call (nothing
    // changes the environment this early).
    let value = unsafe { libc::getenv(name.as_ptr()) };
    if value.is_null() {
        return default;
    }
    // SAFETY: as above.
    unsafe { std::ffi::CStr::from_ptr(value) }.to_str().ok().and_then(|s| s.parse().ok()).unwrap_or(default)
}

extern "C" fn start() {
    // SAFETY: as in `env_num`.
    let path = unsafe { libc::getenv(c"ASMALLOC_MEMSTAT".as_ptr()) };
    if path.is_null() {
        return;
    }
    // SAFETY: `path` is a NUL-terminated string.
    let fd = unsafe { libc::open(path, libc::O_WRONLY | libc::O_CREAT | libc::O_TRUNC | libc::O_CLOEXEC, 0o644) };
    // SAFETY: a literal path.
    let statm = unsafe { libc::open(c"/proc/self/statm".as_ptr(), libc::O_RDONLY | libc::O_CLOEXEC) };
    let (state, scratch, temp) = (map(size_of::<Sampler>()).cast::<Sampler>(), map(SCRATCH), map(TEMP));
    if fd < 0 || statm < 0 || state.is_null() || scratch.is_null() || temp.is_null() {
        return;
    }
    // SAFETY: `state` is a fresh zeroed mapping big enough for a Sampler, and all-zero bytes
    // are a valid Sampler (integers, arrays of integers and raw pointers); the fields that
    // must not be zero are written here, before anything reads them.
    unsafe {
        let s = &mut *state;
        s.walk.scratch = scratch;
        s.walk.temp = temp;
        s.out.fd = fd;
        s.statm = statm;
        s.start_ms = now_ms();
        s.interval_ms = env_num(c"ASMALLOC_MEMSTAT_MS", 10).max(1);
        PROBE_SIZE.store(env_num(c"ASMALLOC_MEMSTAT_PROBE", 0) as usize, Relaxed);
        if env_num(c"ASMALLOC_MEMSTAT_THP", 0) != 0 {
            // SAFETY: a literal path.
            s.rollup = libc::open(c"/proc/self/smaps_rollup".as_ptr(), libc::O_RDONLY | libc::O_CLOEXEC);
        }
        SAMPLER = state;
    }
    let cpu = env_num(c"ASMALLOC_MEMSTAT_CPU", 14) as usize;
    // SAFETY: plain pthread calls; the attribute and CPU set are initialized before use, and
    // `run` matches the signature pthread_create expects.
    unsafe {
        let mut attr: libc::pthread_attr_t = std::mem::zeroed();
        libc::pthread_attr_init(&mut attr);
        let mut set: libc::cpu_set_t = std::mem::zeroed();
        libc::CPU_SET(cpu, &mut set);
        libc::pthread_attr_setaffinity_np(&mut attr, size_of::<libc::cpu_set_t>(), &set);
        let mut thread: libc::pthread_t = 0;
        if libc::pthread_create(&mut thread, &attr, run, std::ptr::null_mut()) == 0 {
            THREAD = thread;
            libc::atexit(finish);
        }
        libc::pthread_attr_destroy(&mut attr);
    }
}

/// Runs `start` before `main`, when the process is still single-threaded.
#[used]
#[unsafe(link_section = ".init_array")]
static START: extern "C" fn() = start;

/// Bytes the program holds, by the class of the layout it asked with (the last entry: layouts
/// with no class). Kept by [`Counting`], the global allocator of the `asmalloc-memstat` build.
static REQ: [std::sync::atomic::AtomicI64; CLASSES] = [const { std::sync::atomic::AtomicI64::new(0) }; CLASSES];

/// What the sampler has learned of the heap, for [`slot_index`].
static HEAP_BASE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
static REGION_SHIFT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
static REGION_CLASS: [std::sync::atomic::AtomicU8; MAX_REGIONS] =
    [const { std::sync::atomic::AtomicU8::new(0) }; MAX_REGIONS];
static REQ_SLOT: [std::sync::atomic::AtomicI64; CLASSES] = [const { std::sync::atomic::AtomicI64::new(0) }; CLASSES];

/// The class of the slot at `ptr`, as the sampler last saw its region, or the last entry.
fn slot_index(ptr: *mut u8) -> usize {
    let (base, shift) = (HEAP_BASE.load(Relaxed), REGION_SHIFT.load(Relaxed));
    if base == 0 || ptr.addr() < base {
        return CLASSES - 1;
    }
    match REGION_CLASS.get((ptr.addr() - base) >> shift).map_or(0, |c| c.load(Relaxed)) {
        0 => CLASSES - 1,
        c => c as usize - 1,
    }
}

static PROBE_SIZE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
/// [op][log2 size]: op 0 alloc, 1 alloc_zeroed, 2 realloc result, 3 dealloc, 4 realloc source.
static PROBE: [[std::sync::atomic::AtomicU64; 40]; 5] =
    [const { [const { std::sync::atomic::AtomicU64::new(0) }; 40] }; 5];

fn probe(op: usize, ptr: *mut u8, size: usize) {
    let want = PROBE_SIZE.load(Relaxed);
    if want == 0 {
        return;
    }
    let i = slot_index(ptr);
    if i < CLASSES - 1 && Class::from_index(i as u8).map_or(0, Class::size) == want {
        PROBE[op][(usize::BITS - size.max(1).leading_zeros()) as usize % 40].fetch_add(1, Relaxed);
    }
}

/// Blocks the program holds in slots of the probed class, as far as the probe saw them.
fn probe_live() -> i64 {
    let sum = |op: usize| PROBE[op].iter().map(|n| n.load(Relaxed) as i64).sum::<i64>();
    sum(0) + sum(1) + sum(2) - sum(3) - sum(4)
}

fn req_index(layout: std::alloc::Layout) -> usize {
    Class::of_layout(layout).map_or(CLASSES - 1, |c| c.index() as usize)
}

/// asmalloc, counting what the program asks for by class, so that a sample can set the bytes
/// asked for against the slots that hold them.
pub struct Counting;

// SAFETY: every call goes to asmalloc unchanged; the counting touches no block.
unsafe impl std::alloc::GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: std::alloc::Layout) -> *mut u8 {
        // SAFETY: the caller's promise, passed on.
        let p = unsafe { asmalloc::Asmalloc.alloc(layout) };
        if !p.is_null() {
            REQ[req_index(layout)].fetch_add(layout.size() as i64, Relaxed);
            REQ_SLOT[slot_index(p)].fetch_add(layout.size() as i64, Relaxed);
            probe(0, p, layout.size());
        }
        p
    }

    unsafe fn alloc_zeroed(&self, layout: std::alloc::Layout) -> *mut u8 {
        // SAFETY: the caller's promise, passed on.
        let p = unsafe { asmalloc::Asmalloc.alloc_zeroed(layout) };
        if !p.is_null() {
            REQ[req_index(layout)].fetch_add(layout.size() as i64, Relaxed);
            REQ_SLOT[slot_index(p)].fetch_add(layout.size() as i64, Relaxed);
            probe(1, p, layout.size());
        }
        p
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: std::alloc::Layout) {
        REQ[req_index(layout)].fetch_sub(layout.size() as i64, Relaxed);
        REQ_SLOT[slot_index(ptr)].fetch_sub(layout.size() as i64, Relaxed);
        probe(3, ptr, layout.size());
        // SAFETY: the caller's promise, passed on.
        unsafe { asmalloc::Asmalloc.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: std::alloc::Layout, new: usize) -> *mut u8 {
        // SAFETY: the caller's promise, passed on.
        let p = unsafe { asmalloc::Asmalloc.realloc(ptr, layout, new) };
        if !p.is_null() {
            REQ[req_index(layout)].fetch_sub(layout.size() as i64, Relaxed);
            REQ_SLOT[slot_index(ptr)].fetch_sub(layout.size() as i64, Relaxed);
            probe(4, ptr, layout.size());
            REQ_SLOT[slot_index(p)].fetch_add(new as i64, Relaxed);
            probe(2, p, new);
            // SAFETY: the caller promises that `new` with the old alignment is a valid layout.
            let next = unsafe { std::alloc::Layout::from_size_align_unchecked(new, layout.align()) };
            REQ[req_index(next)].fetch_add(new as i64, Relaxed);
        }
        p
    }
}
