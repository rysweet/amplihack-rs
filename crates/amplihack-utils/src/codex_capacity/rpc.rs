//! One live exchange with `codex app-server` (Unix only).
//!
//! The child runs in its own process group with `CODEX_HOME` set to the
//! account directory, and `OPENAI_API_KEY`, `CODEX_API_KEY` and `RUST_LOG`
//! removed. amplihack writes `initialize` (id 1), waits for its answer, then
//! writes the `initialized` notification and `account/rateLimits/read`
//! (id 2) and waits for that answer.
//!
//! One deadline covers the whole read. Its last [`STOP_RESERVE`] (or a
//! quarter of it, if that is less) is kept for stopping the app server, so
//! the start, the handshake and the answer must be done by the earlier
//! *ask deadline*. The app server may be renewing the account's login when
//! the read ends, and a renewal killed between retiring the old refresh
//! token and writing the new one can leave the account signed out. So on
//! every path the stop is graceful first, then forced:
//!
//! 1. stdin is closed. The app server exits on its own when its stdin ends
//!    (codex-cli 0.166.0 exits with status 0 about half a second later).
//! 2. If it has not exited by the middle of the stop window, its process
//!    group gets `SIGTERM`.
//! 3. If it has still not exited by the deadline, the group gets `SIGKILL`.
//!
//! Each wait is for the exit itself: a thread blocks in `waitid(WNOWAIT)`
//! and sends on a channel when the child exits; the waits are
//! `recv_timeout` up to the step's deadline, with no polling. The child is
//! reaped only after that event, and the group is always killed after the
//! exit as well, so nothing the app server started outlives the read.
//! What a stop cannot cover is an app server that ignores both the end of
//! its stdin and `SIGTERM` until the deadline; that one is killed, and if
//! it was mid-renewal the account may have to sign in again.
//!
//! Stdout is read on a thread, line by line, each line capped at 1 MiB.
//! Stderr is read on another to its end, keeping its last 64 KiB. The
//! reader threads are not joined: a process that left the group and kept
//! a pipe open cannot hold the read past its deadline.

use super::Outcome;
use super::parse;
use serde_json::{Value, json};
use std::ffi::OsStr;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

/// The longest stdout line read; a longer one is an unexpected response.
const LINE_CAP: usize = 1024 * 1024;
/// The stderr bytes kept (the last ones).
const STDERR_CAP: usize = 64 * 1024;
/// The end of the deadline kept for stopping the app server gracefully
/// (at most a quarter of the deadline). The same 3 seconds the Claude
/// reader keeps before it will send a login refresh.
pub(super) const STOP_RESERVE: Duration = Duration::from_secs(3);
const UNEXPECTED: &str = "unexpected response from codex app-server";

/// The part of `timeout` kept for the stop: [`STOP_RESERVE`], or a
/// quarter of `timeout` when that is less.
pub(super) fn stop_reserve(timeout: Duration) -> Duration {
    STOP_RESERVE.min(timeout / 4)
}

/// The process-group id to signal for a child `pid`: `-pid`, but never for
/// pid 0 (our own group) or 1 (`kill(-1)` is every process), nor for a pid
/// that does not fit a `pid_t`.
pub(super) fn group_target(pid: u32) -> Option<i32> {
    i32::try_from(pid).ok().filter(|p| *p > 1).map(|p| -p)
}

/// Blocks until child `pid` has exited, without reaping it, then sends on
/// the returned channel. The child stays waitable for `Child::wait`.
fn watch_exit(pid: u32) -> Receiver<()> {
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        let id = libc::id_t::from(pid);
        // SAFETY: an all-zero `siginfo_t` is a valid value for waitid to fill.
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        loop {
            // SAFETY: `info` is a valid, writable siginfo_t; `id` is our own
            // child, which is not reaped before this thread reports (see
            // `Guard::reap`). WNOWAIT leaves it waitable for `Child::wait`.
            let rc =
                unsafe { libc::waitid(libc::P_PID, id, &mut info, libc::WEXITED | libc::WNOWAIT) };
            if rc == 0 || std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted
            {
                // Exited, or (ECHILD) nothing left to wait for.
                break;
            }
        }
        let _ = tx.send(());
    });
    rx
}

/// Owns the child: stops its group and reaps it, when dropped if not
/// before.
struct Guard {
    child: Child,
    exit: Receiver<()>,
    exited: bool,
    reaped: bool,
}

impl Guard {
    fn new(child: Child) -> Self {
        let exit = watch_exit(child.id());
        Self {
            child,
            exit,
            exited: false,
            reaped: false,
        }
    }

    fn signal_group(&self, sig: libc::c_int) {
        if let Some(target) = group_target(self.child.id()) {
            // SAFETY: `kill` has no memory-safety preconditions. The child
            // is not reaped yet, so its pid (the group id) is still ours.
            // ESRCH (an empty group) is expected and ignored.
            let _ = unsafe { libc::kill(target, sig) };
        }
    }

    /// Waits until `until` for the child's exit; whether it has exited.
    fn wait_exit(&mut self, until: Instant) -> bool {
        if !self.exited {
            let left = until.saturating_duration_since(Instant::now());
            // A hang-up means the watcher is gone, which it is only after
            // reporting; count it as the exit.
            self.exited = !matches!(self.exit.recv_timeout(left), Err(RecvTimeoutError::Timeout));
        }
        self.exited
    }

    /// Stops the app server (stdin must already be closed): waits for its
    /// own exit until the middle of the window ending at `until`, then
    /// sends the group `SIGTERM` and waits until `until`, then kills the
    /// group. The group is killed after the exit too, for anything the
    /// app server left running. Gives the child's exit status.
    fn stop(&mut self, until: Instant) -> Option<ExitStatus> {
        let now = Instant::now();
        let mid = now + until.saturating_duration_since(now) / 2;
        if !self.wait_exit(mid) {
            self.signal_group(libc::SIGTERM);
            self.wait_exit(until);
        }
        self.reap()
    }

    /// Kills the group, waits for the exit event, then reaps the child.
    fn reap(&mut self) -> Option<ExitStatus> {
        self.signal_group(libc::SIGKILL);
        if !self.exited {
            let _ = self.child.kill();
            // SIGKILL cannot be caught: the exit comes. Waiting for the
            // watcher before reaping keeps its waitid off a reused pid.
            let _ = self.exit.recv();
            self.exited = true;
        }
        let status = self.child.wait().ok();
        self.reaped = true;
        status
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        if !self.reaped {
            self.reap();
        }
    }
}

/// What the stdout thread sends.
enum Event {
    Line(String),
    TooLong,
    Eof,
}

/// Reads one account. See the module docs.
pub(super) fn read(bin: &OsStr, home: &Path, timeout: Duration) -> Outcome {
    let deadline = Instant::now() + timeout;
    let ask_deadline = deadline - stop_reserve(timeout);
    let mut cmd = Command::new(bin);
    cmd.arg("app-server")
        .env("CODEX_HOME", home)
        .env_remove("OPENAI_API_KEY")
        .env_remove("CODEX_API_KEY")
        .env_remove("RUST_LOG")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Outcome::Error(format!(
                "codex binary not found: {}",
                Path::new(bin).display()
            ));
        }
        Err(e) => {
            return Outcome::Error(format!("cannot start {}: {e}", Path::new(bin).display()));
        }
    };
    let (stdin, stdout, stderr) = (child.stdin.take(), child.stdout.take(), child.stderr.take());
    let mut guard = Guard::new(child);
    let (Some(stdin), Some(stdout), Some(stderr)) = (stdin, stdout, stderr) else {
        return Outcome::Error("cannot start codex app-server: no pipes".to_string());
    };
    let lines = spawn_stdout(stdout);
    let (tail, stderr_done) = spawn_stderr(stderr);
    let mut ex = Exchange {
        stdin: Some(stdin),
        lines,
        deadline: ask_deadline,
    };

    let step = ex.converse();
    // Closing stdin is the app server's signal to finish and exit.
    drop(ex);
    match step {
        Step::Done(o) => {
            guard.stop(stop_until(deadline, timeout));
            o
        }
        Step::Closed if guard.wait_exit(ask_deadline) => {
            let status = guard.stop(deadline);
            exited(status, deadline, &tail, &stderr_done)
        }
        Step::Closed => {
            guard.stop(deadline);
            Outcome::Timeout
        }
    }
}

/// When a stop that starts now must end: the deadline, but no more than
/// the stop reserve away, so a read that got its answer early does not
/// wait the rest of the deadline for an app server that will not exit.
fn stop_until(deadline: Instant, timeout: Duration) -> Instant {
    deadline.min(Instant::now() + stop_reserve(timeout))
}

/// How a conversation ended.
enum Step {
    Done(Outcome),
    /// Stdout closed before the answer.
    Closed,
}

struct Exchange {
    stdin: Option<ChildStdin>,
    lines: Receiver<Event>,
    deadline: Instant,
}

impl Exchange {
    fn converse(&mut self) -> Step {
        let init = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {"clientInfo": {"name": "amplihack", "version": env!("CARGO_PKG_VERSION")}}
        });
        self.send(&[init]);
        match self.answer(1) {
            Answer::Result(_) => {}
            Answer::Error(o) => return Step::Done(o),
            Answer::End(s) => return s,
        }
        let note = json!({"jsonrpc": "2.0", "method": "initialized"});
        let req = json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "account/rateLimits/read",
            "params": {}
        });
        self.send(&[note, req]);
        match self.answer(2) {
            Answer::Result(v) => Step::Done(match parse::reading(&v) {
                Some(r) => Outcome::Ok(r),
                None => Outcome::Error(UNEXPECTED.to_string()),
            }),
            Answer::Error(o) => Step::Done(o),
            Answer::End(s) => s,
        }
    }

    /// Writes `msgs`, one line each. A failed write (the child closed its
    /// stdin or exited) is not reported here: the reply never comes, and
    /// the read ends with the exit or the deadline.
    fn send(&mut self, msgs: &[Value]) {
        let mut text = String::new();
        for m in msgs {
            text.push_str(&m.to_string());
            text.push('\n');
        }
        if let Some(stdin) = self.stdin.as_mut()
            && (stdin.write_all(text.as_bytes()).is_err() || stdin.flush().is_err())
        {
            self.stdin = None;
        }
    }

    /// Waits for the answer with `id`, skipping every other line.
    fn answer(&mut self, id: u64) -> Answer {
        loop {
            let left = self.deadline.saturating_duration_since(Instant::now());
            let line = match self.lines.recv_timeout(left) {
                Ok(Event::Line(l)) => l,
                Ok(Event::TooLong) => {
                    return Answer::End(Step::Done(Outcome::Error(UNEXPECTED.to_string())));
                }
                Ok(Event::Eof) | Err(RecvTimeoutError::Disconnected) => {
                    return Answer::End(Step::Closed);
                }
                Err(RecvTimeoutError::Timeout) => return Answer::End(Step::Done(Outcome::Timeout)),
            };
            let Ok(v) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if v.get("id").and_then(Value::as_u64) != Some(id) {
                continue;
            }
            if let Some(err) = v.get("error").filter(|e| !e.is_null()) {
                let msg = err.get("message").and_then(Value::as_str).unwrap_or("");
                return Answer::Error(if parse::is_not_signed_in(msg) {
                    Outcome::NotSignedIn
                } else if msg.trim().is_empty() {
                    Outcome::Error(UNEXPECTED.to_string())
                } else {
                    Outcome::Error(parse::clean(msg))
                });
            }
            return match v.get("result") {
                Some(r) => Answer::Result(r.clone()),
                None => Answer::Error(Outcome::Error(UNEXPECTED.to_string())),
            };
        }
    }
}

enum Answer {
    Result(Value),
    Error(Outcome),
    End(Step),
}

/// Stdout closed and the child exited before the answer: names its exit
/// status and its last non-empty stderr line. Stderr has ended once the
/// group is gone, unless a process that left the group holds it; it is
/// waited for until the deadline.
fn exited(
    status: Option<ExitStatus>,
    deadline: Instant,
    tail: &Arc<Mutex<Vec<u8>>>,
    stderr_done: &Receiver<()>,
) -> Outcome {
    let _ = stderr_done.recv_timeout(deadline.saturating_duration_since(Instant::now()));
    let bytes = tail.lock().unwrap_or_else(PoisonError::into_inner).clone();
    let text = String::from_utf8_lossy(&bytes);
    let last = text
        .lines()
        .rev()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map(parse::clean);
    let status = status.map_or_else(|| "unknown status".to_string(), |s| s.to_string());
    Outcome::Error(match last {
        Some(l) => format!("codex app-server exited ({status}): {l}"),
        None => format!("codex app-server exited ({status})"),
    })
}

/// Reads stdout lines onto a channel until EOF, a read error or a line
/// longer than [`LINE_CAP`].
fn spawn_stdout(out: impl Read + Send + 'static) -> Receiver<Event> {
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        let mut r = BufReader::new(out);
        loop {
            let mut buf = Vec::new();
            let read = r
                .by_ref()
                .take(LINE_CAP as u64 + 1)
                .read_until(b'\n', &mut buf);
            // A read error ends stdout as EOF does: the reply cannot come,
            // and the caller reports the exit (with its stderr) or the
            // deadline.
            if !matches!(read, Ok(n) if n > 0) {
                let _ = tx.send(Event::Eof);
                return;
            }
            if buf.last() == Some(&b'\n') {
                buf.pop();
            } else if buf.len() > LINE_CAP {
                let _ = tx.send(Event::TooLong);
                return;
            }
            let line = String::from_utf8_lossy(&buf).into_owned();
            if tx.send(Event::Line(line)).is_err() {
                return;
            }
        }
    });
    rx
}

/// Reads stderr to its end, keeping the last [`STDERR_CAP`] bytes; the
/// receiver gets `()` (or a hang-up) when it has ended.
fn spawn_stderr(err: impl Read + Send + 'static) -> (Arc<Mutex<Vec<u8>>>, Receiver<()>) {
    let tail = Arc::new(Mutex::new(Vec::new()));
    let (tx, rx) = channel();
    let keep = Arc::clone(&tail);
    std::thread::spawn(move || {
        let mut err = err;
        let mut chunk = [0u8; 8192];
        loop {
            match err.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => {
                    let mut t = keep.lock().unwrap_or_else(PoisonError::into_inner);
                    t.extend_from_slice(&chunk[..n]);
                    if t.len() > STDERR_CAP {
                        let cut = t.len() - STDERR_CAP;
                        t.drain(..cut);
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(_) => break,
            }
        }
        let _ = tx.send(());
    });
    (tail, rx)
}
