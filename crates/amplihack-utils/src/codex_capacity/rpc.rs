//! One live exchange with `codex app-server` (Unix only).
//!
//! The child runs in its own process group with `CODEX_HOME` set to the
//! account directory, and `OPENAI_API_KEY`, `CODEX_API_KEY` and `RUST_LOG`
//! removed. amplihack writes `initialize` (id 1), waits for its answer, then
//! writes the `initialized` notification and `account/rateLimits/read`
//! (id 2) and waits for that answer. One deadline covers the start, the
//! handshake and the answer.
//!
//! Stdout is read on a thread, line by line, each line capped at 1 MiB.
//! Stderr is read on another to its end, keeping its last 64 KiB. On every
//! path [`Guard`] kills the whole process group, then kills and reaps the
//! child, so nothing the app server started outlives the read. The reader
//! threads are not joined: a process that left the group and kept a pipe
//! open cannot hold the read past its deadline.

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
/// How often an exit is polled for once stdout has closed.
const POLL: Duration = Duration::from_millis(10);
/// How long to wait for stderr to end after the group is killed.
const STDERR_GRACE: Duration = Duration::from_millis(500);
const UNEXPECTED: &str = "unexpected response from codex app-server";

/// The process-group id to signal for a child `pid`: `-pid`, but never for
/// pid 0 (our own group) or 1 (`kill(-1)` is every process), nor for a pid
/// that does not fit a `pid_t`.
pub(super) fn group_target(pid: u32) -> Option<i32> {
    i32::try_from(pid).ok().filter(|p| *p > 1).map(|p| -p)
}

/// Kills the child's group, then kills and reaps the child, when dropped.
struct Guard {
    child: Child,
    reaped: bool,
}

impl Guard {
    fn kill_group(&self) {
        if let Some(target) = group_target(self.child.id()) {
            // SAFETY: `kill` has no memory-safety preconditions. The child
            // is not reaped yet, so its pid (the group id) is still ours.
            // ESRCH (an empty group) is expected and ignored.
            let _ = unsafe { libc::kill(target, libc::SIGKILL) };
        }
    }

    /// Whether the child has exited, without reaping it.
    fn exited(&self) -> bool {
        let pid = libc::id_t::from(self.child.id());
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
                // SAFETY: waitid succeeded, so `si_pid` is initialised; it
                // is 0 while the child is still running (WNOHANG).
                return unsafe { info.si_pid() } != 0;
            }
            if std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted {
                // ECHILD: nothing to wait for, so it is gone.
                return true;
            }
        }
    }

    /// Kills the group and reaps the child, giving its exit status.
    fn finish(&mut self) -> Option<ExitStatus> {
        self.kill_group();
        let _ = self.child.kill();
        let status = self.child.wait().ok();
        self.reaped = true;
        status
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        if !self.reaped {
            self.finish();
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
    let mut guard = Guard {
        child,
        reaped: false,
    };
    let (Some(stdin), Some(stdout), Some(stderr)) = (stdin, stdout, stderr) else {
        return Outcome::Error("cannot start codex app-server: no pipes".to_string());
    };
    let lines = spawn_stdout(stdout);
    let (tail, stderr_done) = spawn_stderr(stderr);
    let mut ex = Exchange {
        stdin: Some(stdin),
        lines,
        deadline,
    };

    let outcome = match ex.converse() {
        Step::Done(o) => o,
        Step::Closed => exited(&mut guard, deadline, &tail, &stderr_done),
    };
    // Close stdin before the group is killed by the guard.
    drop(ex);
    outcome
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

/// Stdout closed before the answer: waits (within the deadline) for the
/// exit, kills the group, reaps the child and names its status and its
/// last non-empty stderr line.
fn exited(
    guard: &mut Guard,
    deadline: Instant,
    tail: &Arc<Mutex<Vec<u8>>>,
    stderr_done: &Receiver<()>,
) -> Outcome {
    while !guard.exited() {
        if Instant::now() >= deadline {
            return Outcome::Timeout;
        }
        std::thread::sleep(POLL);
    }
    let status = guard.finish();
    let _ = stderr_done.recv_timeout(STDERR_GRACE);
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
