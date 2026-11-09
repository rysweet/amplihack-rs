//! Issue #2095: a test-spawned rotator ends with the test process that
//! started it. Production never arms this; it changes nothing outside tests.
//!
//! # Why test rotators leaked
//!
//! `issue_1592_daemon.rs`, `issue_1746_daily_archive_rotator.rs` and
//! `issue_1592_root_rotator.rs` run `profile rotate start`, which goes
//! through `daemon::start` → `launch_core` → `daemon::spawn`. `spawn` starts
//! `profile rotate watch … --spawned` with `setsid`, so the watcher is in its
//! own session, is reparented to init when `start` exits, and is not tied to
//! the test process in any way the kernel enforces.
//!
//! - A test that completes stops its rotator with `profile rotate stop`
//!   (`stop_until_gone`), so it does not leak.
//! - A test that panics does not leak either: unwinding runs the `Reaper` or
//!   `Guard` drops, which signal the pid the fixture's pid file names. The
//!   one gap is `issue_1592_root_rotator.rs`, where the pid file is read and
//!   parsed after `start` returns but before `Guard` exists; a panic there
//!   leaves the watcher running.
//! - SIGKILL, `abort` or a cancelled `cargo test` runs no destructor, so the
//!   watcher outlives the test and keeps cycling against a deleted temporary
//!   root.
//!
//! # The mechanism
//!
//! The harness creates a pipe, keeps the write end (close-on-exec) and lets
//! only the child it starts inherit the read end, naming its number in
//! [`LIFETIME_FD_ENV`]. `spawn` passes that fd and variable on to the
//! watcher, and `watch` calls [`arm`]. A thread blocks on the read end; when
//! the test process ends for any reason the kernel closes the write end, the
//! read returns end of file, and the thread sets the watcher's existing stop
//! flag. The watcher then leaves by its SIGTERM path: probes are cancelled,
//! refreshes finish and the pid file is removed.
//!
//! `PR_SET_PDEATHSIG` was rejected because it fires when the `start` CLI
//! exits, not when the test exits. A child subreaper was rejected because it
//! would change the whole multi-threaded test binary.
//!
//! The `start` CLI keeps its inherited, non-close-on-exec read end, and its
//! children may inherit it. That is harmless: a read end cannot delay end of
//! file. Do not "fix" it by passing the write end instead, which would keep
//! the pipe open after the test died.
//!
//! The module prints and logs nothing, so the rotate log grammar is
//! unchanged and an unarmed watcher behaves exactly as before.

use std::ffi::OsStr;
use std::fs::File;
use std::io::{self, Read};
use std::os::fd::{FromRawFd, RawFd};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};

/// Names the inherited read end of the test's lifetime pipe. It must not
/// start with `AMPLIHACK_ROTATE`, or `spawn` would strip it.
pub(super) const LIFETIME_FD_ENV: &str = "AMPLIHACK_TEST_ROTATOR_LIFETIME_FD";

/// Set to `1` by the test harness only; production never sets it.
const TEST_GUARD_ENV: &str = "AMPLIHACK_TEST_HOME_GUARD";

/// Returns the fd to watch when every arming condition holds: the variable
/// is UTF-8 that parses, untrimmed, as an `i32` of at least 3; the guard is
/// exactly `"1"`; and `inherited_fifo(fd)` is true. The predicate is not
/// called unless the variable and the guard pass.
pub(super) fn armed_fd(
    var: Option<&OsStr>,
    guard: Option<&OsStr>,
    inherited_fifo: impl FnOnce(RawFd) -> bool,
) -> Option<RawFd> {
    let fd: RawFd = var?.to_str()?.parse().ok()?;
    if fd < 3 || guard? != OsStr::new("1") {
        return None;
    }
    inherited_fifo(fd).then_some(fd)
}

/// True only for an open FIFO whose `FD_CLOEXEC` flag is clear. A pipe this
/// process opened through std is close-on-exec, so this tells an fd inherited
/// across `execve` apart from one that merely reuses the same number.
pub(super) fn inherited_fifo(fd: RawFd) -> bool {
    if fd < 0 {
        return false;
    }
    let mut st = std::mem::MaybeUninit::<libc::stat>::uninit();
    // SAFETY: fstat writes into `st` and does nothing else; a closed fd
    // returns -1 with EBADF.
    if unsafe { libc::fstat(fd, st.as_mut_ptr()) } != 0 {
        return false;
    }
    // SAFETY: fstat returned 0, so it filled `st`.
    let mode = unsafe { st.assume_init() }.st_mode;
    if mode & libc::S_IFMT != libc::S_IFIFO {
        return false;
    }
    cloexec_clear(fd)
}

fn cloexec_clear(fd: RawFd) -> bool {
    // SAFETY: F_GETFD only reads the descriptor flags.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    flags != -1 && flags & libc::FD_CLOEXEC == 0
}

/// Arms the lifetime pipe when this watcher was started by the test harness.
/// On any failure it returns silently and the watcher runs as it does today.
pub(super) fn arm(stop: &Arc<AtomicBool>) {
    let var = std::env::var_os(LIFETIME_FD_ENV);
    let guard = std::env::var_os(TEST_GUARD_ENV);
    let Some(fd) = armed_fd(var.as_deref(), guard.as_deref(), inherited_fifo) else {
        return;
    };
    if !cloexec_clear(fd) {
        return;
    }
    // Probe children must not hold the read end.
    // SAFETY: F_SETFD only changes the descriptor flags of `fd`.
    if unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) } == -1 {
        return;
    }
    // SAFETY: `fd` was an open FIFO with FD_CLOEXEC clear at the time of the
    // check, which no fd this process opens has, so nothing else owns it.
    let file = unsafe { File::from_raw_fd(fd) };
    // A thread that cannot start leaves the watcher unarmed; the harness's
    // own stop path still applies. The handle is dropped to detach it.
    if let Ok(handle) = spawn_watch(file, Arc::clone(stop)) {
        drop(handle);
    }
}

fn spawn_watch<R>(reader: R, stop: Arc<AtomicBool>) -> io::Result<JoinHandle<()>>
where
    R: Read + Send + 'static,
{
    thread::Builder::new()
        .name("rotate-lifetime".into())
        .spawn(move || pump(reader, &stop))
}

/// Reads until end of file or an error other than `Interrupted`, ignoring
/// any data, then sets `stop`.
fn pump<R: Read>(mut reader: R, stop: &AtomicBool) {
    let mut buf = [0u8; 64];
    loop {
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(_) => {}
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(_) => break,
        }
    }
    stop.store(true, Ordering::SeqCst);
}

#[cfg(test)]
fn watch_pipe<R>(reader: R, stop: Arc<AtomicBool>) -> JoinHandle<()>
where
    R: Read + Send + 'static,
{
    thread::spawn(move || pump(reader, &stop))
}

#[cfg(test)]
#[path = "tests_lifetime.rs"]
mod tests;
