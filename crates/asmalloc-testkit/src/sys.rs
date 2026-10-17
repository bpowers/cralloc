//! The few process calls the fork test needs, declared directly against the C library so the
//! kit needs no dependency.

use core::ffi::{c_int, c_void};
use std::time::{Duration, Instant};

unsafe extern "C" {
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
