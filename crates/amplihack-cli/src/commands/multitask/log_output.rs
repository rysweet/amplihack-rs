//! Workstream subprocess log-output handling.

use super::utils::tail_output;
use std::path::PathBuf;
use std::process::Child;
use std::sync::{Arc, Mutex};
use std::thread;

pub(super) fn spawn_log_output_thread(
    child: Arc<Mutex<Option<Child>>>,
    log_file: PathBuf,
    issue: i64,
) {
    let max_log_bytes: u64 = std::env::var("AMPLIHACK_MAX_LOG_BYTES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(100 * 1024 * 1024);

    thread::spawn(move || {
        let mut child_guard = child.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(ref mut child) = *child_guard
            && let Some(stdout) = child.stdout.take()
        {
            drop(child_guard);
            tail_output(stdout, &log_file, issue, max_log_bytes);
        }
    });
}
