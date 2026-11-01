//! Every resident page of the process at exit, for any allocator (feature `memacct`; method and
//! results in docs/research/memory.md, "Small heaps").
//!
//! With `BENCH_MEMACCT=<file>` set, an exit handler writes to the file:
//!
//! - `self <start> <len>` for each of its own buffers, which a reader leaves out;
//! - `cgroup current=<bytes> peak=<bytes>`, read before anything else here touches memory, so a
//!   reader can tell how far the state at exit is from the peak, and the cgroup's `memory.stat`
//!   (`stat <key>=<value>`), which is current only if `BENCH_MEMACCT_WAIT_MS` held the process
//!   for the kernel's periodic flush (two seconds): a cgroup this small never reaches the
//!   batch that flushes it sooner;
//! - `/proc/self/status` lines for the resident set and page tables;
//! - `/proc/self/smaps`, verbatim, after a `smaps` line;
//! - for every mapping of at most [`PAGEMAP_LIMIT`] bytes with resident pages, one line per run
//!   of present pages that are the process's own anonymous pages (`anon <start> <pages>`), and
//!   per run of pages mapped to the shared zero page by a read (`zero <start> <pages>`: no memory
//!   of their own, but they need page tables), from `/proc/self/pagemap`, which unlike `mincore`
//!   tells a private file page that was written (and so charged to this process) from one that
//!   was only read.
//!
//! `bench/probes/memory/acct.py` turns this, the binary's symbol table and cralloc's `memstat`
//! breakdown into a table by structure. The handler never calls the global allocator: buffers
//! come from `mmap` and text is formatted into a fixed buffer.
use std::fmt::Write as _;

const PAGE: usize = 4096;
/// Mappings larger than this (cralloc's reservation, glibc's arena reservations) are left to
/// smaps' totals and the allocator's own walk.
const PAGEMAP_LIMIT: usize = 1 << 30;
const READ_BUF: usize = 16 << 20;

/// Bit 63 of a pagemap entry: the page is present. Bit 61: it is a file page or shared
/// anonymous; clear for a private anonymous page, including a written copy of a file page.
/// Bit 56: the page is mapped only here, which a page of the process's own is and the shared
/// zero page, mapped by a read of memory never written, is not.
const PRESENT: u64 = 1 << 63;
const FILE_OR_SHARED: u64 = 1 << 61;
const EXCLUSIVE: u64 = 1 << 56;

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

/// Reads a whole file into `buf` (`READ_BUF` bytes) and returns its text, or "" on failure.
fn read_file(path: &std::ffi::CStr, buf: *mut u8) -> &'static str {
    // SAFETY: a NUL-terminated path.
    let fd = unsafe { libc::open(path.as_ptr(), libc::O_RDONLY | libc::O_CLOEXEC) };
    if fd < 0 {
        return "";
    }
    let mut len = 0;
    while len < READ_BUF {
        // SAFETY: reads into the free part of the buffer.
        let n = unsafe { libc::read(fd, buf.add(len).cast(), READ_BUF - len) };
        if n <= 0 {
            break;
        }
        len += n as usize;
    }
    // SAFETY: closes the descriptor opened above.
    unsafe { libc::close(fd) };
    // SAFETY: `len` bytes of the buffer were written; the buffer lives until exit.
    std::str::from_utf8(unsafe { std::slice::from_raw_parts(buf, len) }).unwrap_or("")
}

/// The cgroup v2 directory of this process, as a NUL-terminated path in `out`.
fn cgroup_dir<'a>(text: &str, out: &'a mut [u8; 512]) -> Option<&'a str> {
    let rel = text.lines().find_map(|l| l.strip_prefix("0::"))?.trim();
    let mut w = 0;
    for part in ["/sys/fs/cgroup", rel] {
        let b = part.as_bytes();
        if w + b.len() >= out.len() {
            return None;
        }
        out[w..w + b.len()].copy_from_slice(b);
        w += b.len();
    }
    std::str::from_utf8(&out[..w]).ok()
}

fn cstr_path<'a>(dir: &str, file: &str, out: &'a mut [u8; 600]) -> &'a std::ffi::CStr {
    let mut w = 0;
    for b in dir.bytes().chain("/".bytes()).chain(file.bytes()) {
        if w + 1 >= out.len() {
            break;
        }
        out[w] = b;
        w += 1;
    }
    out[w] = 0;
    std::ffi::CStr::from_bytes_until_nul(&out[..=w]).unwrap_or(c"")
}

/// Writes runs of present private anonymous pages of `[start, end)`.
fn anon_runs(out: &mut Out, pagemap: i32, start: usize, end: usize, entries: *mut u64) {
    let pages = (end - start) / PAGE;
    let cap = READ_BUF / 8;
    let mut done = 0;
    // The current run: its kind, first page and length.
    let mut run: Option<(&str, usize, usize)> = None;
    while done < pages {
        let n = (pages - done).min(cap);
        let offset = ((start / PAGE + done) * 8) as libc::off_t;
        // SAFETY: reads at most `n` entries into a buffer of `cap >= n` entries.
        let got = unsafe { libc::pread(pagemap, entries.cast(), n * 8, offset) };
        if got <= 0 {
            break;
        }
        let got = got as usize / 8;
        // SAFETY: `got` entries were just written.
        let slice = unsafe { std::slice::from_raw_parts(entries, got) };
        for (i, &e) in slice.iter().enumerate() {
            let page = start + (done + i) * PAGE;
            let kind = if e & PRESENT == 0 || e & FILE_OR_SHARED != 0 {
                None
            } else if e & EXCLUSIVE != 0 {
                Some("anon")
            } else {
                Some("zero")
            };
            run = match (run, kind) {
                (Some((k, s, len)), Some(now)) if k == now && s + len * PAGE == page => Some((k, s, len + 1)),
                (previous, now) => {
                    if let Some((k, s, len)) = previous {
                        let _ = writeln!(out, "{k} {s:x} {len}");
                    }
                    now.map(|k| (k, page, 1))
                }
            };
        }
        done += got;
    }
    if let Some((k, s, len)) = run {
        let _ = writeln!(out, "{k} {s:x} {len}");
    }
}

fn hex(s: &str) -> usize {
    usize::from_str_radix(s, 16).unwrap_or(0)
}

extern "C" fn dump() {
    // SAFETY: as in `start`, the variable outlives the call.
    let path = unsafe { libc::getenv(c"BENCH_MEMACCT".as_ptr()) };
    if path.is_null() {
        return;
    }
    // SAFETY: `path` is a NUL-terminated string.
    let fd = unsafe { libc::open(path, libc::O_WRONLY | libc::O_CREAT | libc::O_TRUNC | libc::O_CLOEXEC, 0o644) };
    let (text_buf, smaps_buf, entries) = (map(READ_BUF), map(READ_BUF), map(READ_BUF).cast::<u64>());
    if fd < 0 || text_buf.is_null() || smaps_buf.is_null() || entries.is_null() {
        return;
    }
    // The kernel flushes the cgroup's statistics every two seconds, so a wait of that long
    // makes `memory.stat` describe this process.
    // SAFETY: as for BENCH_MEMACCT.
    let wait = unsafe { libc::getenv(c"BENCH_MEMACCT_WAIT_MS".as_ptr()) };
    if !wait.is_null() {
        // SAFETY: a NUL-terminated string.
        let ms: u64 = unsafe { std::ffi::CStr::from_ptr(wait) }.to_str().ok().and_then(|s| s.parse().ok()).unwrap_or(0);
        let ts =
            libc::timespec { tv_sec: (ms / 1000) as libc::time_t, tv_nsec: (ms % 1000 * 1_000_000) as libc::c_long };
        // SAFETY: nanosleep reads one timespec.
        unsafe { libc::nanosleep(&ts, std::ptr::null_mut()) };
    }
    let mut out = Out { fd, len: 0, buf: [0; 4096] };
    for start in [text_buf.addr(), smaps_buf.addr(), entries.addr()] {
        let _ = writeln!(out, "self {start:x} {READ_BUF}");
    }
    let mut dir_buf = [0u8; 512];
    let mut path_buf = [0u8; 600];
    let cgroup = read_file(c"/proc/self/cgroup", text_buf);
    if let Some(dir) = cgroup_dir(cgroup, &mut dir_buf) {
        let dir: &str = dir;
        let current = read_file(cstr_path(dir, "memory.current", &mut path_buf), text_buf).trim().to_owned_fixed();
        let peak = read_file(cstr_path(dir, "memory.peak", &mut path_buf), text_buf).trim().to_owned_fixed();
        let _ = writeln!(out, "cgroup current={} peak={}", current.as_str(), peak.as_str());
        for line in read_file(cstr_path(dir, "memory.stat", &mut path_buf), text_buf).lines() {
            if let Some((k, v)) = line.split_once(' ') {
                let _ = writeln!(out, "stat {k}={v}");
            }
        }
    }
    for line in read_file(c"/proc/self/status", text_buf).lines() {
        if ["VmRSS", "RssAnon", "RssFile", "VmPTE", "VmHWM", "Threads"].iter().any(|k| line.starts_with(k)) {
            let _ = writeln!(out, "status {}", line.split_whitespace().collect::<Fixed>().as_str());
        }
    }
    let smaps = read_file(c"/proc/self/smaps", smaps_buf);
    let _ = writeln!(out, "smaps");
    let _ = out.write_str(smaps);
    let _ = writeln!(out, "end-smaps");
    // SAFETY: a literal path.
    let pagemap = unsafe { libc::open(c"/proc/self/pagemap".as_ptr(), libc::O_RDONLY | libc::O_CLOEXEC) };
    if pagemap >= 0 {
        let (mut range, mut rss) = (None::<(usize, usize)>, 0usize);
        let flush = |out: &mut Out, range: Option<(usize, usize)>, rss: usize| {
            if let Some((s, e)) = range
                && rss > 0
                && e - s <= PAGEMAP_LIMIT
            {
                anon_runs(out, pagemap, s, e, entries);
            }
        };
        for line in smaps.lines() {
            let first = line.split_whitespace().next().unwrap_or("");
            if let Some((s, e)) = first.split_once('-')
                && !first.ends_with(':')
            {
                flush(&mut out, range, rss);
                range = Some((hex(s), hex(e)));
                rss = 0;
            } else if let Some(v) = line.strip_prefix("Rss:") {
                rss = v.trim().trim_end_matches("kB").trim().parse().unwrap_or(0);
            }
        }
        flush(&mut out, range, rss);
    }
    out.flush();
}

/// A short string held in place, so that nothing here calls the global allocator.
struct Fixed {
    len: usize,
    buf: [u8; 256],
}

impl Fixed {
    fn as_str(&self) -> &str {
        std::str::from_utf8(&self.buf[..self.len]).unwrap_or("")
    }
}

impl<'a> FromIterator<&'a str> for Fixed {
    fn from_iter<I: IntoIterator<Item = &'a str>>(iter: I) -> Fixed {
        let mut f = Fixed { len: 0, buf: [0; 256] };
        for (i, part) in iter.into_iter().enumerate() {
            for b in (if i > 0 { " " } else { "" }).bytes().chain(part.bytes()) {
                if f.len < f.buf.len() {
                    f.buf[f.len] = b;
                    f.len += 1;
                }
            }
        }
        f
    }
}

trait ToFixed {
    fn to_owned_fixed(&self) -> Fixed;
}

impl ToFixed for str {
    fn to_owned_fixed(&self) -> Fixed {
        std::iter::once(self).collect()
    }
}

extern "C" fn start() {
    // SAFETY: getenv before main, when nothing changes the environment.
    if !unsafe { libc::getenv(c"BENCH_MEMACCT".as_ptr()) }.is_null() {
        // SAFETY: registers a handler with the C library's exit.
        unsafe { libc::atexit(dump) };
    }
}

/// Runs `start` before `main`. The handler it registers runs after `main` returns, after every
/// handler registered later (cralloc's memstat sampler, if any, has then written its own).
#[used]
#[unsafe(link_section = ".init_array")]
static START: extern "C" fn() = start;
