//! Condition waits with one generous ceiling (issue #1973).
//!
//! Tests wait on a condition, never on a fixed delay. Every wait ends at
//! [`CEILING`] at the latest and then fails with a message naming the
//! condition and the time spent, plus at most the last 4 KiB of a child's
//! stderr. The environment of a child is never printed. A child that is
//! still running at the ceiling is killed and reaped.
//!
//! This file uses only `std`, so other test crates may include it with
//! `#[path]`.

#![allow(dead_code)]

use std::io::Read;
use std::process::{Child, Command, ExitStatus, Output, Stdio};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// The longest any test waits for a condition: long enough to outlast a
/// host-wide stall, short enough that a real hang still fails.
pub const CEILING: Duration = Duration::from_secs(600);

/// How often every wait here checks its condition.
pub const POLL: Duration = Duration::from_millis(50);
const TAIL: usize = 4096;
const BUSY_RETRIES: u32 = 50;
const BUSY_PAUSE: Duration = Duration::from_millis(20);

/// Polls `cond` every 50 ms until it holds, for at most [`CEILING`].
pub fn wait_until(what: &str, cond: impl FnMut() -> bool) {
    wait_until_with_ceiling(what, CEILING, cond);
}

/// [`wait_until`] with the ceiling as a parameter.
pub fn wait_until_with_ceiling(what: &str, ceiling: Duration, mut cond: impl FnMut() -> bool) {
    let start = Instant::now();
    loop {
        if cond() {
            return;
        }
        let spent = start.elapsed();
        if spent >= ceiling {
            panic!("waiting for {what}: timed out after {spent:?}");
        }
        std::thread::sleep(POLL.min(ceiling - spent));
    }
}

/// Runs `cmd` to completion with stdout and stderr captured, for at most
/// [`CEILING`]. The caller's stdio settings for stdout and stderr are
/// replaced by pipes; stdin is left as the caller set it.
pub fn output_within(what: &str, cmd: &mut Command) -> Output {
    output_with_ceiling(what, cmd, CEILING)
}

/// [`output_within`] with the ceiling as a parameter.
pub fn output_with_ceiling(what: &str, cmd: &mut Command, ceiling: Duration) -> Output {
    let start = Instant::now();
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = spawn_retrying(what, cmd);
    let stdout = Arc::new(Mutex::new(Vec::new()));
    let stderr = Arc::new(Mutex::new(Vec::new()));
    let out_reader = drain(child.stdout.take(), Arc::clone(&stdout));
    let err_reader = drain(child.stderr.take(), Arc::clone(&stderr));
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {}
            Err(e) => {
                kill_and_reap(&mut child);
                panic!("waiting for {what}: cannot poll the child: {e}");
            }
        }
        let spent = start.elapsed();
        if spent >= ceiling {
            kill_and_reap(&mut child);
            // The readers are not joined: a grandchild may still hold a pipe.
            panic!(
                "waiting for {what}: timed out after {spent:?}; the child was killed\n\
                 stderr tail:\n{}",
                tail(&stderr)
            );
        }
        std::thread::sleep(POLL.min(ceiling - spent));
    };
    // The child has exited; its output ends once every holder of the pipes
    // has closed them.
    wait_until_with_ceiling(
        &format!("the output of {what} to end"),
        ceiling.saturating_sub(start.elapsed()).max(POLL),
        || out_reader.is_finished() && err_reader.is_finished(),
    );
    for reader in [out_reader, err_reader] {
        if let Err(e) = reader.join() {
            std::panic::resume_unwind(e);
        }
    }
    Output {
        status,
        stdout: take(&stdout),
        stderr: take(&stderr),
    }
}

/// Waits for `child` to exit, for at most [`CEILING`].
pub fn wait_child(what: &str, child: &mut Child) -> ExitStatus {
    wait_child_with_ceiling(what, child, CEILING)
}

/// [`wait_child`] with the ceiling as a parameter. At the ceiling the child
/// is killed and reaped before the panic.
pub fn wait_child_with_ceiling(what: &str, child: &mut Child, ceiling: Duration) -> ExitStatus {
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status,
            Ok(None) => {}
            Err(e) => {
                kill_and_reap(child);
                panic!("waiting for {what}: cannot poll the child: {e}");
            }
        }
        let spent = start.elapsed();
        if spent >= ceiling {
            kill_and_reap(child);
            panic!("waiting for {what}: timed out after {spent:?}; the child was killed");
        }
        std::thread::sleep(POLL.min(ceiling - spent));
    }
}

/// Spawns `cmd`, retrying while the program is busy being written
/// (`ETXTBSY`, when a sibling test's fork briefly holds a write descriptor).
pub fn spawn_retrying(what: &str, cmd: &mut Command) -> Child {
    let mut tries = 0;
    loop {
        match cmd.spawn() {
            Ok(child) => return child,
            Err(e)
                if e.kind() == std::io::ErrorKind::ExecutableFileBusy && tries < BUSY_RETRIES =>
            {
                tries += 1;
                std::thread::sleep(BUSY_PAUSE);
            }
            Err(e) => panic!("waiting for {what}: cannot start the program: {e}"),
        }
    }
}

fn drain<R: Read + Send + 'static>(src: Option<R>, into: Arc<Mutex<Vec<u8>>>) -> JoinHandle<()> {
    std::thread::spawn(move || {
        let Some(mut src) = src else { return };
        let mut buf = [0u8; 8192];
        loop {
            match src.read(&mut buf) {
                Ok(0) => return,
                Ok(n) => lock(&into).extend_from_slice(&buf[..n]),
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) => panic!("reading a child's output failed: {e}"),
            }
        }
    })
}

fn lock(buf: &Mutex<Vec<u8>>) -> std::sync::MutexGuard<'_, Vec<u8>> {
    buf.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn take(buf: &Mutex<Vec<u8>>) -> Vec<u8> {
    std::mem::take(&mut *lock(buf))
}

fn tail(buf: &Mutex<Vec<u8>>) -> String {
    tail_of(&lock(buf))
}

/// At most the last 4 KiB of `bytes`, for a timeout message.
pub fn tail_of(bytes: &[u8]) -> String {
    let from = bytes.len().saturating_sub(TAIL);
    String::from_utf8_lossy(&bytes[from..]).into_owned()
}

/// Kills `child` and reaps it. Kills only the child this process owns.
pub fn kill_and_reap(child: &mut Child) {
    // An error here means the child has already exited; `wait` reaps it.
    let _ = child.kill();
    if let Err(e) = child.wait() {
        eprintln!("reaping a killed child failed: {e}");
    }
}
