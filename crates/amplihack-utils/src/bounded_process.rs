//! Running a helper command with a time limit and a cap on its output.
//!
//! [`run`] uses no shell, gives stdin `/dev/null`, keeps at most `cap` bytes
//! from each of stdout and stderr, and stops the command at the time limit
//! or when `cancel` is set. On Unix the child leads its own process group,
//! and the whole group is killed when the run ends, so processes the command
//! started (MCP servers, for example) do not outlive it.
//!
//! The pipes are read on detached threads, so a process that keeps a pipe
//! open never blocks the caller. Used by `amplihack profile usage`, the
//! detached recipe launch and `amplihack workstreams status` (issue #1710).

use std::io::Read;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

const POLL: Duration = Duration::from_millis(20);

/// The limits for one run.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// How long the command may run.
    pub timeout: Duration,
    /// Bytes kept from each of stdout and stderr; the rest is read and
    /// dropped so the command cannot stall on a full pipe.
    pub cap: usize,
    /// How long the pipes may stay open after the command has exited.
    pub read_grace: Duration,
}

/// Why a run produced no exit status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Failure {
    /// The command could not be started.
    Start,
    /// Its pipes could not be taken.
    Pipes,
}

/// How the command ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ended {
    /// It exited. `None` when it exited but could not be reaped.
    Exited(Option<ExitStatus>),
    /// It was still running at the time limit or when `cancel` was set.
    Stopped,
    /// Waiting for it failed; it has been killed.
    WaitFailed,
}

/// What a run left.
#[derive(Debug)]
pub struct Output {
    pub ended: Ended,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    /// Both pipes reached end of file without a read error. Only then is the
    /// output known to be whole. Always `false` unless the command exited.
    pub complete: bool,
}

impl Output {
    /// Whether the command exited with status 0.
    pub fn success(&self) -> bool {
        matches!(self.ended, Ended::Exited(Some(s)) if s.success())
    }
}

/// Runs `cmd` as described in the module comment. `cancel`, when given, is
/// polled with the clock and stops the command as [`Ended::Stopped`].
pub fn run(
    mut cmd: Command,
    limits: Limits,
    cancel: Option<&AtomicBool>,
) -> Result<Output, Failure> {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    let start = Instant::now();
    let mut child = cmd.spawn().map_err(|_| Failure::Start)?;
    let (Some(out), Some(err)) = (child.stdout.take(), child.stderr.take()) else {
        stop(&mut child);
        return Err(Failure::Pipes);
    };
    let (out, err) = (
        Stream::spawn(out, limits.cap),
        Stream::spawn(err, limits.cap),
    );
    let stopped = |ended| Output {
        ended,
        stdout: out.take(),
        stderr: err.take(),
        complete: false,
    };
    let cancelled = || cancel.is_some_and(|c| c.load(Ordering::Relaxed));

    let status = loop {
        match exited(&mut child) {
            Ok(true) => {
                // The child is not reaped yet, so its pid (the group id)
                // cannot have been reused: anything it left in its group
                // goes too, then it is reaped.
                kill_group(&child);
                break child.wait().ok();
            }
            Ok(false) if start.elapsed() >= limits.timeout || cancelled() => {
                stop(&mut child);
                return Ok(stopped(Ended::Stopped));
            }
            Ok(false) => std::thread::sleep(POLL),
            Err(_) => {
                stop(&mut child);
                return Ok(stopped(Ended::WaitFailed));
            }
        }
    };
    let exited_at = Instant::now();
    while !(out.done() && err.done()) && exited_at.elapsed() < limits.read_grace {
        std::thread::sleep(Duration::from_millis(10));
    }
    let complete = out.done() && err.done() && !out.failed() && !err.failed();
    Ok(Output {
        ended: Ended::Exited(status),
        stdout: out.take(),
        stderr: err.take(),
        complete,
    })
}

/// Bytes kept from each pipe of a helper command.
const HELPER_CAP: usize = 4 * 1024 * 1024;
/// How long a helper's pipes may stay open after its group is gone.
const HELPER_READ_GRACE: Duration = Duration::from_secs(1);

/// [`run`] for a short helper command (`git`, `gh`, `tmux`) whose stderr is
/// never shown: 4 MiB kept from each pipe, no cancel. `None` when it cannot
/// start or did not exit on its own before `timeout`.
pub fn run_helper(cmd: Command, timeout: Duration) -> Option<Output> {
    let limits = Limits {
        timeout,
        cap: HELPER_CAP,
        read_grace: HELPER_READ_GRACE,
    };
    run(cmd, limits, None)
        .ok()
        .filter(|out| matches!(out.ended, Ended::Exited(_)))
}

/// One pipe, read on a detached thread, keeping at most `cap` bytes.
#[derive(Default)]
struct Stream {
    kept: Mutex<Vec<u8>>,
    done: AtomicBool,
    failed: AtomicBool,
}

impl Stream {
    fn spawn(mut source: impl Read + Send + 'static, cap: usize) -> Arc<Self> {
        let stream = Arc::new(Self::default());
        let shared = Arc::clone(&stream);
        std::thread::spawn(move || {
            let mut chunk = [0u8; 8192];
            loop {
                match source.read(&mut chunk) {
                    Ok(0) => break,
                    Ok(n) => {
                        let mut kept = shared.kept.lock().unwrap_or_else(PoisonError::into_inner);
                        let room = cap.saturating_sub(kept.len());
                        kept.extend_from_slice(&chunk[..n.min(room)]);
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                    Err(_) => {
                        shared.failed.store(true, Ordering::Release);
                        break;
                    }
                }
            }
            shared.done.store(true, Ordering::Release);
        });
        stream
    }

    fn done(&self) -> bool {
        self.done.load(Ordering::Acquire)
    }

    fn failed(&self) -> bool {
        self.failed.load(Ordering::Acquire)
    }

    fn take(&self) -> Vec<u8> {
        std::mem::take(&mut *self.kept.lock().unwrap_or_else(PoisonError::into_inner))
    }
}

/// Kills the child's process group, then kills and reaps the child. These
/// fail only when there is nothing left to kill or reap.
fn stop(child: &mut Child) {
    kill_group(child);
    let _ = child.kill();
    let _ = child.wait();
}

/// Whether the child has exited, without reaping it on Unix, so that
/// [`kill_group`] still targets its own group.
#[cfg(unix)]
fn exited(child: &mut Child) -> std::io::Result<bool> {
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

#[cfg(not(unix))]
fn exited(child: &mut Child) -> std::io::Result<bool> {
    child.try_wait().map(|s| s.is_some())
}

/// Sends SIGKILL to the process group the child leads. The group id is the
/// child's pid, and every caller runs this before the child is reaped, so
/// the pid cannot have been reused and this cannot reach another group.
#[cfg(unix)]
fn kill_group(child: &Child) {
    let Ok(pid) = i32::try_from(child.id()) else {
        return;
    };
    // SAFETY: `kill` has no memory-safety preconditions. A negative pid
    // targets the group; ESRCH (group already empty) is ignored.
    let _ = unsafe { libc::kill(-pid, libc::SIGKILL) };
}

#[cfg(not(unix))]
fn kill_group(_child: &Child) {}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn limits(timeout_ms: u64, cap: usize) -> Limits {
        Limits {
            timeout: Duration::from_millis(timeout_ms),
            cap,
            read_grace: Duration::from_secs(1),
        }
    }

    fn sh(script: &str) -> Command {
        let mut c = Command::new("/bin/sh");
        c.args(["-c", script]);
        c
    }

    #[test]
    fn exit_status_and_both_streams_are_returned() {
        let out = run(
            sh("printf out; printf err >&2; exit 3"),
            limits(5000, 1024),
            None,
        )
        .unwrap();
        assert!(matches!(out.ended, Ended::Exited(Some(s)) if s.code() == Some(3)));
        assert!(!out.success());
        assert_eq!(out.stdout, b"out");
        assert_eq!(out.stderr, b"err");
        assert!(out.complete);
    }

    #[test]
    fn output_past_the_cap_is_dropped_but_read() {
        let out = run(sh("head -c 100000 /dev/zero"), limits(5000, 10), None).unwrap();
        assert!(out.success());
        assert_eq!(out.stdout.len(), 10);
        assert!(out.complete);
    }

    #[test]
    fn a_command_past_the_limit_is_stopped() {
        let started = Instant::now();
        let out = run(sh("sleep 30"), limits(200, 1024), None).unwrap();
        assert_eq!(out.ended, Ended::Stopped);
        assert!(!out.complete);
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    #[test]
    fn cancel_stops_the_command() {
        let cancel = AtomicBool::new(true);
        let out = run(sh("sleep 30"), limits(30_000, 1024), Some(&cancel)).unwrap();
        assert_eq!(out.ended, Ended::Stopped);
    }

    #[test]
    fn run_helper_returns_only_commands_that_exited() {
        let out = run_helper(sh("printf ok"), Duration::from_secs(5)).unwrap();
        assert!(out.success());
        assert_eq!(out.stdout, b"ok");
        assert!(run_helper(sh("sleep 30"), Duration::from_millis(200)).is_none());
        assert!(run_helper(Command::new("/nonexistent/x"), Duration::from_secs(1)).is_none());
    }

    #[test]
    fn a_missing_program_is_a_start_failure() {
        let cmd = Command::new("/nonexistent/amplihack-bounded-process-test");
        assert_eq!(
            run(cmd, limits(1000, 16), None).unwrap_err(),
            Failure::Start
        );
    }

    #[test]
    fn a_background_process_holding_the_pipe_is_killed_with_the_group() {
        // The shell exits at once; its background `sleep` keeps stdout open
        // and would block a reader that waited for end of file.
        let started = Instant::now();
        let out = run(sh("sleep 30 & printf done"), limits(5000, 1024), None).unwrap();
        assert!(out.success());
        assert_eq!(out.stdout, b"done");
        assert!(
            out.complete,
            "group kill closes the pipe within the grace period"
        );
        assert!(started.elapsed() < Duration::from_secs(10));
    }
}
