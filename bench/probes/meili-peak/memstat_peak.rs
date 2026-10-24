//! Where asmalloc's resident memory is, sampled while a workload runs: the meili-peak variant
//! of bench-alloc's memstat (bench/probes/meili-peak/peak_edit.py copies it over memstat.rs in a
//! scratch tree; docs/research/meili-peak.md). Not product code.
//!
//! With `ASMALLOC_MEMSTAT=<file>` set, a thread started before `main` wakes every
//! `ASMALLOC_MEMSTAT_MS` milliseconds (default 250), walks the heap with `Asmalloc::inspect`,
//! asks the kernel which pages of every range the walk names are resident (`mincore`), and
//! writes to `<file>.<pid>` one block a sample:
//!
//! - `at`: the process's anonymous memory (statm's resident less shared), the sampler's own
//!   resident buffers, glibc's arenas (`mallinfo2`), and asmalloc's resident memory by use:
//!   `live` (slots the program holds), `loaded`/`spare`/`depot` (objects parked in magazines),
//!   `free` (free in a bitmap on a resident page) with the large classes' part split by their
//!   region's hold (`flyoung`: stamped within the hold, `flold`: past it, `flnone`: no stamp),
//!   `above` (resident above a watermark), metadata, magazines, slabs; `req`, the bytes the
//!   program holds as it asked for them;
//! - `atc`: the same by class, with what the program asked for in that class's slots;
//! - `atd`: the allocator's diagnostic counters (`asmalloc::diag`), index=value, non-zero only.
//!
//! The sampler never calls the global allocator: its thread comes from `pthread_create`, its
//! buffers from `mmap`, and it formats into a fixed buffer. The walk is a racy snapshot.
//! It pins itself to `ASMALLOC_MEMSTAT_CPU` (default 13).
use asmalloc::classes::{Class, MagazineSize, SmallClass};
use asmalloc::stats::{Place, Region, Visitor};
use std::fmt::Write as _;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicUsize, Ordering::*};

const PAGE: usize = 4096;
const CLASSES: usize = asmalloc::classes::COUNT + 1;
const MAX_REGIONS: usize = 4096;
/// One byte per page of every region: enough for 128 GiB of committed regions.
const SCRATCH: usize = 32 << 20;
/// One byte per page for ranges looked at once: 4 GiB at a time.
const TEMP: usize = 1 << 20;
const SMALL_MAX: usize = 32 << 10;

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
    meta_res: u64,
}

#[derive(Clone, Copy)]
struct Snap {
    t_ms: u64,
    anon: u64,
    self_res: u64,
    /// glibc's mallinfo2: arena, hblkhd, uordblks, fordblks, keepcost.
    glibc: [u64; 5],
    req: [i64; CLASSES],
    req_slot: [i64; CLASSES],
    retained: u64,
    classes: [ClassStat; CLASSES],
    /// Resident free bytes of large classes by their region's hold: within it, past it, none.
    free_large: [u64; 3],
    runs_res: u64,
    meta_res: u64,
    mags_res: u64,
    slabs_res: u64,
}

impl Snap {
    const ZERO: Snap = Snap {
        t_ms: 0,
        anon: 0,
        self_res: 0,
        glibc: [0; 5],
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
            meta_res: 0,
        }; CLASSES],
        free_large: [0; 3],
        runs_res: 0,
        meta_res: 0,
        mags_res: 0,
        slabs_res: 0,
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
    by_index: [u16; MAX_REGIONS],
    heap_base: usize,
    region_shift: u32,
    current: usize,
    /// The hold age of the region last reported, for a large class.
    current_age: u32,
}

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
        c.carved += region.carved.saturating_sub(region.origin) as u64;
        c.committed += region.committed.saturating_sub(region.origin) as u64;
        if let Some(from) = region.huge_from {
            c.huge_part += region.committed.saturating_sub(from) as u64;
        }
        self.current_age = if region.class.is_small() || region.index >= asmalloc::diag::REGIONS {
            u32::MAX
        } else {
            asmalloc::diag::REGION_AGE[region.index].load(Relaxed)
        };
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
    }

    fn free(&mut self, class: Class, start: *const u8, len: usize) {
        let resident = if self.current == usize::MAX { 0 } else { self.overlap(self.current, start.addr(), len) };
        let (first, end) = (start.addr().next_multiple_of(PAGE), (start.addr() + len) & !(PAGE - 1));
        let whole =
            if self.current == usize::MAX || first >= end { 0 } else { self.overlap(self.current, first, end - first) };
        if !class.is_small() {
            let hold = asmalloc::back::policy::HOLD_STEPS;
            let k = match self.current_age {
                u32::MAX => 2,
                a if a < hold => 0,
                _ => 1,
            };
            self.snap.free_large[k] += resident;
        }
        let c = &mut self.snap.classes[class.index() as usize];
        c.free += len as u64;
        c.free_res += resident;
        c.free_whole += whole;
    }

    fn run(&mut self, base: *const u8, len: usize, _size: usize) {
        self.snap.runs_res += self.resident_once(base.addr(), len);
    }

    fn magazine_area(&mut self, base: *const u8, _carved: usize, committed: usize) {
        self.snap.mags_res = self.resident_once(base.addr(), committed);
    }

    fn slabs(&mut self, base: *const u8, bytes: usize, count: usize) {
        self.snap.slabs_res = self.resident_once(base.addr(), bytes * count);
    }

    fn magazine(&mut self, _place: Place, _class: SmallClass, _objects: usize) {}

    fn cached(&mut self, place: Place, class: SmallClass, object: *const u8) {
        let size = class.size();
        let resident = self.record_of(object.addr()).map_or(0, |rec| self.overlap(rec, object.addr(), size));
        let k = match place {
            Place::Loaded(_) => 0,
            Place::Spare(_) => 1,
            Place::Depot => 2,
        };
        let c = &mut self.snap.classes[class.class().index() as usize];
        c.cached[k] += size as u64;
        c.cached_res[k] += resident;
    }

    fn empties(&mut self, _size: MagazineSize, _count: usize) {}
}

/// A fixed buffer that formats without allocating and writes to a file descriptor.
struct Out {
    fd: i32,
    len: usize,
    buf: [u8; 8192],
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

#[repr(C)]
#[derive(Default)]
struct Mallinfo2 {
    arena: usize,
    ordblks: usize,
    smblks: usize,
    hblks: usize,
    hblkhd: usize,
    usmblks: usize,
    fsmblks: usize,
    uordblks: usize,
    fordblks: usize,
    keepcost: usize,
}

unsafe extern "C" {
    fn mallinfo2() -> Mallinfo2;
}

struct Sampler {
    walk: Walk,
    /// The sample with the most anonymous memory.
    peak: Snap,
    out: Out,
    statm: i32,
    start_ms: u64,
    interval_ms: u64,
    state_at: usize,
}

static STOP: AtomicBool = AtomicBool::new(false);
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
    /// Anonymous resident bytes: statm's resident less its shared (file and shmem) pages.
    fn anon(&self) -> u64 {
        let mut buf = [0u8; 128];
        // SAFETY: pread writes at most the buffer's length.
        let n = unsafe { libc::pread(self.statm, buf.as_mut_ptr().cast(), buf.len(), 0) };
        let text = std::str::from_utf8(&buf[..n.max(0) as usize]).unwrap_or("");
        let mut f = text.split(' ').map(|s| s.trim().parse::<u64>().unwrap_or(0));
        let (_, resident, shared) = (f.next(), f.next().unwrap_or(0), f.next().unwrap_or(0));
        resident.saturating_sub(shared) * PAGE as u64
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
        // The sampler's own buffers are anonymous memory of the process.
        let (scratch, temp, state) = (w.scratch.addr(), w.temp.addr(), self.state_at);
        w.snap.self_res = w.resident_once(scratch, SCRATCH)
            + w.resident_once(temp, TEMP)
            + w.resident_once(state, size_of::<Sampler>());
        self.walk.snap.anon = self.anon();
        // SAFETY: glibc's mallinfo2 takes no arguments and returns a struct by value.
        let mi = unsafe { mallinfo2() };
        self.walk.snap.glibc = [mi.arena, mi.hblkhd, mi.uordblks, mi.fordblks, mi.keepcost].map(|v| v as u64);
        for i in 0..CLASSES {
            let (mut a, mut b) = (0i64, 0i64);
            for s in &SHARDS {
                a += s.req[i].load(Relaxed);
                b += s.req_slot[i].load(Relaxed);
            }
            self.walk.snap.req[i] = a;
            self.walk.snap.req_slot[i] = b;
        }
        let snap = self.walk.snap;
        if snap.anon > self.peak.anon {
            self.peak = snap;
        }
        Sampler::dump(&mut self.out, "at", &snap);
        self.out.flush();
    }

    fn dump(out: &mut Out, label: &str, snap: &Snap) {
        let u = snap.uses();
        let g = snap.glibc;
        let _ = writeln!(
            out,
            "{label} t_ms={} anon={} self={} g_arena={} g_mmap={} g_used={} g_free={} g_top={} req={} heap={} live={} loaded={} spare={} depot={} free={} flyoung={} flold={} flnone={} above={} runs={} meta={} mags={} slabs={} retained={}",
            snap.t_ms,
            snap.anon,
            snap.self_res,
            g[0],
            g[1],
            g[2],
            g[3],
            g[4],
            snap.req.iter().sum::<i64>(),
            snap.heap(),
            u[0],
            u[1],
            u[2],
            u[3],
            u[4],
            snap.free_large[0],
            snap.free_large[1],
            snap.free_large[2],
            u[5],
            snap.runs_res,
            snap.meta_res,
            snap.mags_res,
            snap.slabs_res,
            snap.retained
        );
        for (index, c) in snap.classes.iter().enumerate() {
            if c.regions == 0 && snap.req[index] == 0 && snap.req_slot[index] == 0 {
                continue;
            }
            let size = Class::from_index(index as u8).map_or(0, Class::size);
            let cached: u64 = c.cached_res.iter().sum();
            let _ = writeln!(
                out,
                "{label}c size={size} req={} req_slot={} regions={} carved={} committed={} huge={} resident={} live={} loaded={} spare={} depot={} free={} free_res={} free_whole={} above={} meta={}",
                snap.req[index],
                snap.req_slot[index],
                c.regions,
                c.carved,
                c.committed,
                c.huge_part,
                c.resident,
                c.resident.saturating_sub(c.above + c.free_res + cached),
                c.cached_res[0],
                c.cached_res[1],
                c.cached_res[2],
                c.free,
                c.free_res,
                c.free_whole,
                c.above,
                c.meta_res
            );
        }
        let _ = write!(out, "{label}d");
        for (i, v) in asmalloc::diag::ASMALLOC_DIAG.iter().enumerate() {
            let v = v.load(Relaxed);
            if v != 0 {
                let _ = write!(out, " {i}={v}");
            }
        }
        let _ = writeln!(out);
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
    let peak = sampler.peak;
    Sampler::dump(&mut sampler.out, "peak", &peak);
    sampler.out.flush();
}

fn env_num(name: &std::ffi::CStr, default: u64) -> u64 {
    // SAFETY: getenv returns null or a NUL-terminated string that outlives this call.
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
    // The process ID appended, so that every run keeps its own file.
    let mut named = [0u8; 600];
    // SAFETY: `path` is a NUL-terminated string.
    let given = unsafe { std::ffi::CStr::from_ptr(path) }.to_bytes();
    let mut suffix = [0u8; 24];
    let mut digits = [0u8; 20];
    let (mut pid, mut nd) = (std::process::id(), 0);
    loop {
        digits[nd] = b'0' + (pid % 10) as u8;
        nd += 1;
        pid /= 10;
        if pid == 0 {
            break;
        }
    }
    suffix[0] = b'.';
    for i in 0..nd {
        suffix[1 + i] = digits[nd - 1 - i];
    }
    let slen = 1 + nd;
    let n = given.len().min(named.len() - slen - 1);
    named[..n].copy_from_slice(&given[..n]);
    named[n..n + slen].copy_from_slice(&suffix[..slen]);
    // SAFETY: `named` is NUL-terminated (zeroed past the name).
    let fd = unsafe {
        libc::open(named.as_ptr().cast(), libc::O_WRONLY | libc::O_CREAT | libc::O_TRUNC | libc::O_CLOEXEC, 0o644)
    };
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
        s.interval_ms = env_num(c"ASMALLOC_MEMSTAT_MS", 250).max(1);
        s.state_at = state.addr();
        SAMPLER = state;
    }
    let cpu = env_num(c"ASMALLOC_MEMSTAT_CPU", 13) as usize;
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

/// What the sampler has learned of the heap, for [`slot_index`].
static HEAP_BASE: AtomicUsize = AtomicUsize::new(0);
static REGION_SHIFT: AtomicUsize = AtomicUsize::new(0);
static REGION_CLASS: [std::sync::atomic::AtomicU8; MAX_REGIONS] =
    [const { std::sync::atomic::AtomicU8::new(0) }; MAX_REGIONS];

/// The bytes the program holds, by the class of the layout it asked with (`req`) and by the
/// class of the slot that holds them (`req_slot`, the last entry: not yet known), in shards
/// so that threads do not share the counters' lines.
#[repr(align(128))]
struct Shard {
    req: [AtomicI64; CLASSES],
    req_slot: [AtomicI64; CLASSES],
}

const NSHARDS: usize = 64;
static SHARDS: [Shard; NSHARDS] = [const {
    Shard { req: [const { AtomicI64::new(0) }; CLASSES], req_slot: [const { AtomicI64::new(0) }; CLASSES] }
}; NSHARDS];
static NEXT_SHARD: AtomicUsize = AtomicUsize::new(0);

thread_local! {
    static MY_SHARD: std::cell::Cell<usize> = const { std::cell::Cell::new(usize::MAX) };
}

fn shard() -> &'static Shard {
    let i = MY_SHARD.with(|s| {
        let mut i = s.get();
        if i == usize::MAX {
            i = NEXT_SHARD.fetch_add(1, Relaxed) % NSHARDS;
            s.set(i);
        }
        i
    });
    &SHARDS[i]
}

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

fn req_index(layout: std::alloc::Layout) -> usize {
    Class::of_layout(layout).map_or(CLASSES - 1, |c| c.index() as usize)
}

fn count(layout: std::alloc::Layout, ptr: *mut u8, sign: i64) {
    let s = shard();
    s.req[req_index(layout)].fetch_add(sign * layout.size() as i64, Relaxed);
    s.req_slot[slot_index(ptr)].fetch_add(sign * layout.size() as i64, Relaxed);
}

/// asmalloc, counting what the program asks for by class.
pub struct Counting;

const _: () = assert!(SMALL_MAX == 32 << 10);

// SAFETY: every call goes to asmalloc unchanged; the counting touches no block.
unsafe impl std::alloc::GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: std::alloc::Layout) -> *mut u8 {
        // SAFETY: the caller's promise, passed on.
        let p = unsafe { asmalloc::Asmalloc.alloc(layout) };
        if !p.is_null() {
            count(layout, p, 1);
        }
        p
    }

    unsafe fn alloc_zeroed(&self, layout: std::alloc::Layout) -> *mut u8 {
        // SAFETY: the caller's promise, passed on.
        let p = unsafe { asmalloc::Asmalloc.alloc_zeroed(layout) };
        if !p.is_null() {
            count(layout, p, 1);
        }
        p
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: std::alloc::Layout) {
        count(layout, ptr, -1);
        // SAFETY: the caller's promise, passed on.
        unsafe { asmalloc::Asmalloc.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: std::alloc::Layout, new: usize) -> *mut u8 {
        // SAFETY: the caller's promise, passed on.
        let p = unsafe { asmalloc::Asmalloc.realloc(ptr, layout, new) };
        if !p.is_null() {
            count(layout, ptr, -1);
            // SAFETY: the caller promises that `new` with the old alignment is a valid layout.
            let next = unsafe { std::alloc::Layout::from_size_align_unchecked(new, layout.align()) };
            count(next, p, 1);
        }
        p
    }
}
