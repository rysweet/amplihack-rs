//! Issue #1710: running a short helper command (`git`, `gh`, `tmux`) with
//! hard limits, for the detached launch and `workstreams status`.
//!
//! [`run_bounded`] uses no shell, gives stdin `/dev/null`, puts the child in
//! its own process group, keeps at most [`PIPE_CAP`] bytes from each pipe,
//! and kills the whole group when the limit passes.

use std::io::Read;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

/// Bytes kept from each of a subprocess's stdout and stderr.
const PIPE_CAP: usize = 4 * 1024 * 1024;
/// How long the pipes may stay open after the process group is gone.
const READ_GRACE: Duration = Duration::from_secs(1);
const POLL: Duration = Duration::from_millis(20);

/// What a bounded subprocess left.
pub(crate) struct Bounded {
    pub success: bool,
    pub stdout: Vec<u8>,
}

/// Runs `cmd` as described in the module comment. `None` when it cannot
/// start, cannot be waited for, or is still running at `timeout`.
pub(crate) fn run_bounded(mut cmd: Command, timeout: Duration) -> Option<Bounded> {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
    let mut child = cmd.spawn().ok()?;
    let out = drain(child.stdout.take());
    // Read so a chatty process cannot fill the pipe and stall; never shown.
    let err = drain(child.stderr.take());
    let start = Instant::now();
    let finished = loop {
        match exited(&child) {
            Ok(true) => break true,
            Ok(false) if start.elapsed() < timeout => std::thread::sleep(POLL),
            _ => break false,
        }
    };
    // The child is not reaped yet, so its pid is still its group's id.
    kill_group(&child);
    let _ = child.kill();
    let status = child.wait().ok();
    if !finished {
        return None;
    }
    let deadline = Instant::now() + READ_GRACE;
    while !(out.done() && err.done()) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    Some(Bounded {
        success: status.is_some_and(|s| s.success()),
        stdout: out.take(),
    })
}

/// A pipe read on a detached thread, keeping at most [`PIPE_CAP`] bytes.
struct Drain {
    kept: Arc<Mutex<Vec<u8>>>,
    done: Arc<AtomicBool>,
}

impl Drain {
    fn done(&self) -> bool {
        self.done.load(std::sync::atomic::Ordering::Acquire)
    }

    fn take(&self) -> Vec<u8> {
        std::mem::take(&mut *self.kept.lock().unwrap_or_else(PoisonError::into_inner))
    }
}

fn drain(source: Option<impl Read + Send + 'static>) -> Drain {
    let kept = Arc::new(Mutex::new(Vec::new()));
    let done = Arc::new(AtomicBool::new(false));
    let Some(mut source) = source else {
        done.store(true, std::sync::atomic::Ordering::Release);
        return Drain { kept, done };
    };
    let (k, d) = (Arc::clone(&kept), Arc::clone(&done));
    std::thread::spawn(move || {
        let mut chunk = [0u8; 8192];
        loop {
            match source.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => {
                    let mut kept = k.lock().unwrap_or_else(PoisonError::into_inner);
                    let room = PIPE_CAP.saturating_sub(kept.len());
                    kept.extend_from_slice(&chunk[..n.min(room)]);
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(_) => break,
            }
        }
        d.store(true, std::sync::atomic::Ordering::Release);
    });
    Drain { kept, done }
}

/// Whether the child has exited, without reaping it, so that
/// [`kill_group`] still targets its own group.
fn exited(child: &std::process::Child) -> std::io::Result<bool> {
    let pid = libc::id_t::from(child.id());
    // SAFETY: an all-zero `siginfo_t` is a valid value for waitid to fill.
    let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
    loop {
        // SAFETY: `info` is a valid, writable siginfo_t; `pid` is our own
        // unreaped child. WNOWAIT leaves it waitable for `Child::wait`.
        let rc = unsafe {
            libc::waitid(
                libc::P_PID,
                pid,
                &mut info,
                libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
            )
        };
        if rc == 0 {
            // SAFETY: waitid succeeded, so `si_pid` is initialised; it is 0
            // when the child has not exited yet (WNOHANG).
            return Ok(unsafe { info.si_pid() } != 0);
        }
        let err = std::io::Error::last_os_error();
        if err.kind() != std::io::ErrorKind::Interrupted {
            return Err(err);
        }
    }
}

/// SIGKILL to the group the unreaped child leads.
fn kill_group(child: &std::process::Child) {
    let Ok(pid) = i32::try_from(child.id()) else {
        return;
    };
    // SAFETY: `kill` has no memory-safety preconditions. A negative pid
    // targets the group; ESRCH (group already empty) is ignored.
    let _ = unsafe { libc::kill(-pid, libc::SIGKILL) };
}
