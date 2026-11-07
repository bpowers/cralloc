//! The few process calls the fork test needs, declared directly against the C library so the
//! kit needs no dependency.

use core::ffi::{c_int, c_void};
use std::time::{Duration, Instant};

unsafe extern "C" {
    fn pthread_attr_init(attr: *mut ThreadAttr) -> c_int;
    fn pthread_attr_setstacksize(attr: *mut ThreadAttr, size: usize) -> c_int;
    fn pthread_attr_destroy(attr: *mut ThreadAttr) -> c_int;
    fn pthread_create(
        thread: *mut usize,
        attr: *const ThreadAttr,
        start: extern "C" fn(*mut c_void) -> *mut c_void,
        arg: *mut c_void,
    ) -> c_int;
    fn pthread_join(thread: usize, value: *mut *mut c_void) -> c_int;
    fn fork() -> c_int;
    fn pipe2(fds: *mut c_int, flags: c_int) -> c_int;
    fn read(fd: c_int, buf: *mut c_void, n: usize) -> isize;
    fn write(fd: c_int, buf: *const c_void, n: usize) -> isize;
    fn close(fd: c_int) -> c_int;
    fn _exit(code: c_int) -> !;
    fn waitpid(pid: c_int, status: *mut c_int, options: c_int) -> c_int;
    fn kill(pid: c_int, sig: c_int) -> c_int;
}

const O_NONBLOCK: c_int = 0o4000;
const O_CLOEXEC: c_int = 0o2_000_000;
const WNOHANG: c_int = 1;
const SIGKILL: c_int = 9;

/// A forked child, seen from the parent.
#[derive(Debug)]
pub struct Child {
    pid: c_int,
    read_fd: c_int,
}

/// The child's end of the pipe to the parent.
#[derive(Debug)]
pub struct Report(c_int);

/// Which side of the fork this is.
pub enum Forked {
    /// The parent, with the child's handle.
    Parent(Child),
    /// The child, with its report pipe.
    Child(Report),
}

/// How the child ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Exit {
    /// It called `_exit` with this code.
    Code(i32),
    /// A signal killed it.
    Signal(i32),
    /// It ran past the timeout and was killed.
    TimedOut,
}

/// Fork, with a non-blocking pipe from child to parent. Panics if either call fails.
pub fn fork_with_pipe() -> Forked {
    let mut fds = [0 as c_int; 2];
    // SAFETY: `fds` has room for the two descriptors `pipe2` writes.
    let rc = unsafe { pipe2(fds.as_mut_ptr(), O_NONBLOCK | O_CLOEXEC) };
    assert_eq!(rc, 0, "pipe2 failed");
    // SAFETY: `fork` has no preconditions. What the child may do afterwards is the caller's
    // concern; the kit's child uses only the subject, fresh model state and the C library.
    let pid = unsafe { fork() };
    assert!(pid >= 0, "fork failed");
    // SAFETY (both arms): each side closes the end it does not use, which it owns.
    if pid == 0 {
        unsafe { close(fds[0]) };
        Forked::Child(Report(fds[1]))
    } else {
        unsafe { close(fds[1]) };
        Forked::Parent(Child {
            pid,
            read_fd: fds[0],
        })
    }
}

impl Report {
    /// Send `bytes` to the parent, as much as the pipe takes without blocking.
    pub fn send(&self, bytes: &[u8]) {
        let mut rest = bytes;
        while !rest.is_empty() {
            // SAFETY: `rest` is valid for reads of its length.
            let n = unsafe { write(self.0, rest.as_ptr().cast(), rest.len()) };
            if n <= 0 {
                return;
            }
            rest = &rest[n as usize..];
        }
    }
}

/// End the child at once, running no destructors or exit handlers: those belong to the
/// parent's process image, including the test harness's.
pub fn exit_now(code: i32) -> ! {
    // SAFETY: `_exit` is always safe to call.
    unsafe { _exit(code) }
}

impl Child {
    /// Wait for the child to end, killing it after `timeout`, and collect what it reported.
    pub fn wait(self, timeout: Duration) -> (Exit, Vec<u8>) {
        let start = Instant::now();
        let mut status: c_int = 0;
        let exit = loop {
            // SAFETY: `status` is a valid out-pointer and `pid` is our child.
            let r = unsafe { waitpid(self.pid, &mut status, WNOHANG) };
            if r == self.pid {
                break decode(status);
            }
            assert!(r == 0, "waitpid failed");
            if start.elapsed() > timeout {
                // SAFETY: as above; the child has not been reaped, so the pid is still ours.
                unsafe {
                    kill(self.pid, SIGKILL);
                    waitpid(self.pid, &mut status, 0);
                }
                break Exit::TimedOut;
            }
            std::thread::sleep(Duration::from_millis(2));
        };
        let mut out = Vec::new();
        let mut buf = [0u8; 4096];
        loop {
            // SAFETY: `buf` is valid for writes of its length.
            let n = unsafe { read(self.read_fd, buf.as_mut_ptr().cast(), buf.len()) };
            if n <= 0 {
                break;
            }
            out.extend_from_slice(&buf[..n as usize]);
        }
        // SAFETY: the parent owns the read end.
        unsafe { close(self.read_fd) };
        (exit, out)
    }
}

fn decode(status: c_int) -> Exit {
    let sig = status & 0x7f;
    if sig == 0 {
        Exit::Code((status >> 8) & 0xff)
    } else {
        Exit::Signal(sig)
    }
}

/// Room for the C library's `pthread_attr_t`, which is 56 bytes on x86-64 Linux with glibc and
/// with musl, and 64 on aarch64 with glibc.
#[repr(C, align(8))]
struct ThreadAttr([u8; 64]);

/// The stack of a child's thread: what the standard library gives its own.
const STACK: usize = 2 << 20;

/// What a thread of [`ChildThreads`] runs.
type Job<'s> = Box<dyn FnOnce() + Send + 's>;

/// Threads for the child of a fork, made with `pthread_create` and not by the standard
/// library.
///
/// The standard library takes a process-wide lock when a thread it made starts and again when
/// it ends (the registry of stacks for its stack-overflow handler). A fork while a thread of
/// the parent holds that lock leaves it locked in the child for ever, and every thread the
/// child then makes through the standard library waits on it before running a line of its
/// closure. The parent of the kit's fork test starts and ends threads all the time, so about
/// one fork in thirty on a busy machine did that, and the child hung. The C library's own
/// thread state is made good by its `fork`.
///
/// Like `std::thread::scope`: every thread is joined before [`ChildThreads::scope`] returns, so
/// a thread may borrow from the caller, and a panic on a thread is raised again from `scope`.
pub struct ChildThreads {
    handles: std::sync::Mutex<Vec<usize>>,
    panicked: std::sync::atomic::AtomicBool,
}

/// A job and the scope to tell if it panics.
struct Start {
    job: Job<'static>,
    panicked: *const std::sync::atomic::AtomicBool,
}

extern "C" fn start(arg: *mut c_void) -> *mut c_void {
    // SAFETY: `arg` is the box `spawn` leaked for this thread alone.
    let start = unsafe { Box::from_raw(arg.cast::<Start>()) };
    let Start { job, panicked } = *start;
    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(job)).is_err() {
        // SAFETY: the scope joins this thread before it goes away.
        unsafe { &*panicked }.store(true, std::sync::atomic::Ordering::Release);
    }
    std::ptr::null_mut()
}

impl ChildThreads {
    /// Runs `f`, which may start threads, and returns once all of them have ended, those
    /// started by other threads included. Panics if a thread panicked.
    pub fn scope<R>(f: impl for<'s> FnOnce(&'s ChildThreads) -> R) -> R {
        /// Joins on the way out, by a panic too: the threads borrow from the caller.
        struct Join<'a>(&'a ChildThreads);
        impl Drop for Join<'_> {
            fn drop(&mut self) {
                // A thread adds the threads it starts before it ends, so the list is empty
                // only when no thread is left to add to it.
                loop {
                    let next = self.0.lock().pop();
                    let Some(handle) = next else { break };
                    // SAFETY: a thread `pthread_create` made, joined once.
                    unsafe { pthread_join(handle, std::ptr::null_mut()) };
                }
            }
        }
        let threads = ChildThreads {
            handles: std::sync::Mutex::new(Vec::new()),
            panicked: std::sync::atomic::AtomicBool::new(false),
        };
        let result = {
            let _join = Join(&threads);
            f(&threads)
        };
        assert!(
            !threads.panicked.load(std::sync::atomic::Ordering::Acquire),
            "a thread of the child panicked"
        );
        result
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<usize>> {
        self.handles
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Starts a thread that runs `job`. Panics if the thread cannot be made.
    pub fn spawn<'s>(&'s self, job: Job<'s>) {
        // SAFETY: only the lifetime changes. The job runs on a thread that `scope` joins
        // before it returns, and `&'s self` is a borrow `scope` handed out, so everything the
        // job borrows outlives its run.
        let job = unsafe { std::mem::transmute::<Job<'s>, Job<'static>>(job) };
        let arg = Box::into_raw(Box::new(Start {
            job,
            panicked: &self.panicked,
        }));
        let mut attr = ThreadAttr([0; 64]);
        let mut handle = 0usize;
        // The list is locked across the call so that the joiner cannot find it empty between
        // this thread's creation and its entry.
        let mut handles = self.lock();
        // SAFETY: `attr` has room for the C library's attribute object and is initialized
        // before use and destroyed after; `start` takes back the box that `arg` leaks.
        let rc = unsafe {
            pthread_attr_init(&mut attr);
            pthread_attr_setstacksize(&mut attr, STACK);
            let rc = pthread_create(&mut handle, &attr, start, arg.cast());
            pthread_attr_destroy(&mut attr);
            rc
        };
        if rc != 0 {
            // SAFETY: no thread was made, so the box is still this thread's.
            drop(unsafe { Box::from_raw(arg) });
            panic!("pthread_create failed: {rc}");
        }
        handles.push(handle);
    }
}
