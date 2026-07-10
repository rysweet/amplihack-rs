//! Idle/liveness watchdog for supervising long-running child processes.
//!
//! Tracking issue: GitHub #867.
//!
//! ## Why this exists
//!
//! Wall-clock, kill-on-expiry timeouts kill healthy agents mid-stream when a
//! task legitimately runs longer than a fixed budget. This module replaces
//! those kills with **idle/liveness detection**: any new byte on `stdout` or
//! `stderr` resets a "last progress" timer, and a child is terminated only
//! after the idle threshold passes with no output.
//!
//! There is **no absolute wall-clock cap** on agentic runs. A live agent that
//! keeps streaming tokens is never killed, regardless of total elapsed time.
//!
//! ## Entry points
//!
//! - [`wait_with_idle_watchdog`] — async, for `tokio::process::Child` (sites 6
//!   and 4: orchestration + remote).
//! - [`wait_with_idle_watchdog_sync`] — blocking, for `std::process::Child`
//!   (site 3: the Copilot CLI client, which must not pull in a tokio runtime).
//! - [`file_idle_since`] — stateless mtime probe for call sites whose child
//!   stdout is already consumed by a logging thread (site 2: multitask).

use std::io;
use std::path::Path;
use std::process::ExitStatus;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

// ---------------------------------------------------------------------------
// Configuration constants
// ---------------------------------------------------------------------------

/// Env var overriding the idle threshold (seconds).
pub const ENV_IDLE_TIMEOUT_SECS: &str = "AMPLIHACK_IDLE_TIMEOUT_SECS";

/// Env var overriding the supervising-loop poll interval (milliseconds).
pub const ENV_IDLE_POLL_MS: &str = "AMPLIHACK_IDLE_POLL_MS";

/// Default idle threshold: a child is killed after this many seconds with no
/// output on either stdout or stderr.
pub const DEFAULT_IDLE_TIMEOUT_SECS: u64 = 300;

/// Default poll interval for the supervising loop (milliseconds).
pub const DEFAULT_POLL_MS: u64 = 1000;

/// Parse a positive `u64` from an environment variable, falling back to
/// `default` on absent, non-parsable, or zero values.
fn env_u64(key: &str, default: u64) -> u64 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(default)
}

// ---------------------------------------------------------------------------
// IdleConfig
// ---------------------------------------------------------------------------

/// Configures the idle threshold and poll interval for a supervised wait.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IdleConfig {
    /// No output for this long → the child is considered idle and is killed.
    pub idle_timeout: Duration,
    /// How often the supervising loop checks for progress and process exit.
    pub poll: Duration,
}

impl IdleConfig {
    /// Reads [`ENV_IDLE_TIMEOUT_SECS`] (default [`DEFAULT_IDLE_TIMEOUT_SECS`])
    /// and [`ENV_IDLE_POLL_MS`] (default [`DEFAULT_POLL_MS`]). Non-parsable or
    /// zero values fall back to the defaults.
    pub fn from_env() -> Self {
        Self {
            idle_timeout: Duration::from_secs(env_u64(
                ENV_IDLE_TIMEOUT_SECS,
                DEFAULT_IDLE_TIMEOUT_SECS,
            )),
            poll: Duration::from_millis(env_u64(ENV_IDLE_POLL_MS, DEFAULT_POLL_MS)),
        }
    }

    /// Override the idle threshold; the poll interval comes from env/default.
    pub fn with_idle(idle: Duration) -> Self {
        Self {
            idle_timeout: idle,
            poll: Duration::from_millis(env_u64(ENV_IDLE_POLL_MS, DEFAULT_POLL_MS)),
        }
    }
}

// ---------------------------------------------------------------------------
// IdleOutcome
// ---------------------------------------------------------------------------

/// Result of a supervised wait.
#[derive(Debug)]
pub struct IdleOutcome {
    /// Exit status of the child, or the I/O error from waiting on it.
    pub status: io::Result<ExitStatus>,
    /// Full captured stdout.
    pub stdout: String,
    /// Full captured stderr.
    pub stderr: String,
    /// True only when the child was killed for exceeding the idle window.
    pub killed_for_idle: bool,
}

/// Shared "last progress" instant, stamped on every read chunk.
type Progress = Arc<Mutex<Instant>>;
/// Shared output buffer accumulated by a drainer.
type Buffer = Arc<Mutex<Vec<u8>>>;

/// Lock a mutex, recovering the inner value even if a drainer panicked.
fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Reclaim a drainer's captured bytes as a `String`, lossily decoding.
///
/// By the time this runs the drainer task/thread has finished, so its `Arc`
/// clone is dropped and `buf`'s strong count is 1. That lets us move the
/// `Vec<u8>` out and hand its allocation directly to `String` — zero-copy for
/// the common valid-UTF-8 case, and no transient second copy of a
/// potentially-multi-megabyte buffer. If the buffer is somehow still shared we
/// fall back to cloning it.
fn take_string(buf: Buffer) -> String {
    let bytes = match Arc::try_unwrap(buf) {
        Ok(m) => m.into_inner().unwrap_or_else(|e| e.into_inner()),
        Err(shared) => lock(&shared).clone(),
    };
    match String::from_utf8(bytes) {
        Ok(s) => s,
        Err(e) => String::from_utf8_lossy(e.as_bytes()).into_owned(),
    }
}

/// Build the final [`IdleOutcome`], reclaiming the captured buffers.
fn outcome(
    status: io::Result<ExitStatus>,
    out_buf: Buffer,
    err_buf: Buffer,
    killed_for_idle: bool,
) -> IdleOutcome {
    IdleOutcome {
        status,
        stdout: take_string(out_buf),
        stderr: take_string(err_buf),
        killed_for_idle,
    }
}

// ---------------------------------------------------------------------------
// Async watchdog (sites 6 & 4)
// ---------------------------------------------------------------------------

/// Read `reader` to EOF, appending each chunk to `buf` and stamping `lp` on
/// every non-empty read so the supervising loop can observe liveness.
async fn drain_async<R>(mut reader: R, buf: Buffer, lp: Progress)
where
    R: tokio::io::AsyncRead + Unpin,
{
    use tokio::io::AsyncReadExt;
    let mut chunk = [0u8; 8192];
    loop {
        match reader.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                lock(&buf).extend_from_slice(&chunk[..n]);
                *lock(&lp) = Instant::now();
            }
        }
    }
}

/// Supervise a `tokio::process::Child`, killing it only after the idle window
/// elapses with no output on either stream.
///
/// Drainer tasks are launched with `tokio::spawn` (requires tokio features
/// `process`, `time`, `rt`, `io-util`). Each read chunk appends to a shared
/// buffer and stamps a shared `Instant`. The supervising loop calls `try_wait`
/// and `sleep(cfg.poll)`; when `now - last_progress >= cfg.idle_timeout` it
/// kills the child and sets `killed_for_idle = true`. `select!` is intentionally
/// not used, so the tokio `macros` feature is not required here.
pub async fn wait_with_idle_watchdog(
    child: &mut tokio::process::Child,
    stdout: Option<tokio::process::ChildStdout>,
    stderr: Option<tokio::process::ChildStderr>,
    cfg: IdleConfig,
) -> IdleOutcome {
    let last_progress: Progress = Arc::new(Mutex::new(Instant::now()));
    let out_buf: Buffer = Arc::new(Mutex::new(Vec::new()));
    let err_buf: Buffer = Arc::new(Mutex::new(Vec::new()));

    let out_handle = stdout.map(|r| {
        tokio::spawn(drain_async(
            r,
            Arc::clone(&out_buf),
            Arc::clone(&last_progress),
        ))
    });
    let err_handle = stderr.map(|r| {
        tokio::spawn(drain_async(
            r,
            Arc::clone(&err_buf),
            Arc::clone(&last_progress),
        ))
    });

    let mut killed_for_idle = false;
    let status: io::Result<ExitStatus> = loop {
        match child.try_wait() {
            Ok(Some(s)) => break Ok(s),
            Ok(None) => {}
            Err(e) => break Err(e),
        }
        tokio::time::sleep(cfg.poll).await;
        if lock(&last_progress).elapsed() >= cfg.idle_timeout {
            let _ = child.start_kill();
            let s = child.wait().await;
            killed_for_idle = true;
            break s;
        }
    };

    // Drain to EOF (the child has exited or been killed, so its pipes close)
    // to capture any output that streamed just before exit.
    if let Some(h) = out_handle {
        let _ = h.await;
    }
    if let Some(h) = err_handle {
        let _ = h.await;
    }

    outcome(status, out_buf, err_buf, killed_for_idle)
}

// ---------------------------------------------------------------------------
// Sync watchdog (site 3)
// ---------------------------------------------------------------------------

/// Blocking sibling of [`drain_async`] for a `std::io::Read` source.
fn drain_sync<R>(mut reader: R, buf: Buffer, lp: Progress)
where
    R: std::io::Read,
{
    let mut chunk = [0u8; 8192];
    loop {
        match reader.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                lock(&buf).extend_from_slice(&chunk[..n]);
                *lock(&lp) = Instant::now();
            }
        }
    }
}

/// Blocking equivalent of [`wait_with_idle_watchdog`] for a
/// `std::process::Child`, implemented with `std::thread` drainers so no tokio
/// runtime is introduced into the caller.
pub fn wait_with_idle_watchdog_sync(
    child: &mut std::process::Child,
    stdout: Option<std::process::ChildStdout>,
    stderr: Option<std::process::ChildStderr>,
    cfg: IdleConfig,
) -> IdleOutcome {
    let last_progress: Progress = Arc::new(Mutex::new(Instant::now()));
    let out_buf: Buffer = Arc::new(Mutex::new(Vec::new()));
    let err_buf: Buffer = Arc::new(Mutex::new(Vec::new()));

    let out_handle = stdout.map(|r| {
        let (buf, lp) = (Arc::clone(&out_buf), Arc::clone(&last_progress));
        std::thread::spawn(move || drain_sync(r, buf, lp))
    });
    let err_handle = stderr.map(|r| {
        let (buf, lp) = (Arc::clone(&err_buf), Arc::clone(&last_progress));
        std::thread::spawn(move || drain_sync(r, buf, lp))
    });

    let mut killed_for_idle = false;
    let status: io::Result<ExitStatus> = loop {
        match child.try_wait() {
            Ok(Some(s)) => break Ok(s),
            Ok(None) => {}
            Err(e) => break Err(e),
        }
        std::thread::sleep(cfg.poll);
        if lock(&last_progress).elapsed() >= cfg.idle_timeout {
            let _ = child.kill();
            let s = child.wait();
            killed_for_idle = true;
            break s;
        }
    };

    if let Some(h) = out_handle {
        let _ = h.join();
    }
    if let Some(h) = err_handle {
        let _ = h.join();
    }

    outcome(status, out_buf, err_buf, killed_for_idle)
}

// ---------------------------------------------------------------------------
// File-mtime idle probe (site 2)
// ---------------------------------------------------------------------------

/// Returns `true` when `path`'s mtime is at least `idle_timeout` old — i.e. no
/// new output has been written to the log file for at least that long.
///
/// This is a stateless probe: callers poll it on their own cadence and kill the
/// child only when it returns `true`.
pub fn file_idle_since(path: &Path, idle_timeout: Duration) -> io::Result<bool> {
    let mtime = std::fs::metadata(path)?.modified()?;
    let age = SystemTime::now()
        .duration_since(mtime)
        .unwrap_or(Duration::ZERO);
    Ok(age >= idle_timeout)
}
