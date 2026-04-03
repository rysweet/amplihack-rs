//! Power-steering re-enable prompt.
//!
//! Ported from `amplihack/power_steering/re_enable_prompt.py`.
//!
//! When power-steering has been temporarily disabled via a `.disabled` file,
//! this module prompts the user on CLI startup with a Y/n choice and a
//! 30-second timeout. The default (YES / timeout) removes the `.disabled`
//! file so that power-steering is re-enabled.
//!
//! ## Integration
//!
//! Called from CLI entry points early in startup via
//! [`prompt_re_enable_if_disabled`].
//!
//! ## Cross-Platform
//!
//! Timeout uses a background thread with a channel. Works on both Unix and
//! Windows without platform-specific signal handling.

use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use thiserror::Error;

use crate::worktree;

/// Default timeout for user response (seconds).
pub const TIMEOUT_SECONDS: u64 = 30;

/// Errors from the power-steering prompt module.
#[derive(Debug, Error)]
pub enum PowerSteeringError {
    /// The worktree module could not resolve the runtime directory.
    #[error("failed to resolve runtime directory: {0}")]
    RuntimeDir(#[from] worktree::WorktreeError),

    /// I/O error interacting with the `.disabled` file.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

/// Result of the re-enable prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReEnableResult {
    /// Power-steering is enabled (was already enabled or user said yes).
    Enabled,
    /// Power-steering remains disabled (user said no).
    Disabled,
}

/// Prompt the user to re-enable power-steering if it is currently disabled.
///
/// Checks for a `.disabled` file in the power-steering directory under the
/// shared runtime directory. If present, prints a prompt and waits up to
/// [`TIMEOUT_SECONDS`] for a response.
///
/// # Returns
///
/// - [`ReEnableResult::Enabled`] — power-steering is (now) enabled.
/// - [`ReEnableResult::Disabled`] — user chose to keep it disabled.
///
/// # Fail-Open
///
/// On any unexpected error the function returns `Enabled` — power-steering
/// should default to being active.
pub fn prompt_re_enable_if_disabled(
    project_root: Option<&Path>,
) -> ReEnableResult {
    match try_prompt(project_root) {
        Ok(r) => r,
        Err(e) => {
            tracing::error!(error = %e, "critical error in prompt_re_enable_if_disabled");
            ReEnableResult::Enabled // fail-open
        }
    }
}

/// Inner implementation that can propagate errors.
fn try_prompt(project_root: Option<&Path>) -> Result<ReEnableResult, PowerSteeringError> {
    let root = match project_root {
        Some(p) => p.to_path_buf(),
        None => std::env::current_dir()?,
    };

    let runtime_dir = match worktree::get_shared_runtime_dir(&root) {
        Ok(d) => PathBuf::from(d),
        Err(e) => {
            tracing::warn!(error = %e, "failed to get shared runtime dir, using fallback");
            root.join(".claude").join("runtime")
        }
    };

    let disabled_file = runtime_dir.join("power-steering").join(".disabled");

    if !disabled_file.exists() {
        return Ok(ReEnableResult::Enabled);
    }

    if is_noninteractive() {
        tracing::info!("non-interactive terminal detected, defaulting to YES");
        remove_disabled_file_safe(&disabled_file, Some("(non-interactive, defaulted to YES)"));
        return Ok(ReEnableResult::Enabled);
    }

    // File exists — prompt user.
    println!("\nPower-Steering is currently disabled.");
    let prompt = "Would you like to re-enable it? [Y/n] (30s timeout, defaults to YES): ";

    match get_input_with_timeout(prompt, TIMEOUT_SECONDS) {
        InputResult::Response(input) => {
            let response = input.trim().to_lowercase();
            if response == "n" || response == "no" {
                println!(
                    "\nPower-Steering remains disabled. You can re-enable it by removing:\n{}\n",
                    disabled_file.display()
                );
                Ok(ReEnableResult::Disabled)
            } else {
                if !response.is_empty() && response != "y" && response != "yes" {
                    tracing::warn!(input = %input.trim(), "invalid input, defaulting to YES");
                }
                remove_disabled_file_safe(&disabled_file, None);
                Ok(ReEnableResult::Enabled)
            }
        }
        InputResult::Timeout => {
            remove_disabled_file_safe(
                &disabled_file,
                Some("(timeout, defaulted to YES)"),
            );
            Ok(ReEnableResult::Enabled)
        }
        InputResult::Interrupted => {
            println!("\n\nPower-Steering remains disabled (user cancelled).\n");
            Ok(ReEnableResult::Disabled)
        }
        InputResult::Eof => {
            tracing::info!("EOF on stdin, defaulting to YES");
            remove_disabled_file_safe(
                &disabled_file,
                Some("(non-interactive, defaulted to YES)"),
            );
            Ok(ReEnableResult::Enabled)
        }
    }
}

// ── input handling ──────────────────────────────────────────────────────

/// Possible outcomes of the timed input prompt.
enum InputResult {
    /// User provided a line of input.
    Response(String),
    /// Timeout elapsed with no input.
    Timeout,
    /// User pressed Ctrl-C.
    Interrupted,
    /// stdin reached EOF (non-interactive).
    Eof,
}

/// Read a line from stdin with a timeout.
///
/// Spawns a thread to read stdin so the main thread can honour the timeout
/// via a channel receive. Works on all platforms.
fn get_input_with_timeout(prompt: &str, timeout_secs: u64) -> InputResult {
    // Print prompt and flush.
    {
        let mut stdout = io::stdout().lock();
        let _ = stdout.write_all(prompt.as_bytes());
        let _ = stdout.flush();
    }

    let (tx, rx) = mpsc::channel();

    std::thread::spawn(move || {
        let stdin = io::stdin();
        let mut line = String::new();
        match stdin.lock().read_line(&mut line) {
            Ok(0) => {
                let _ = tx.send(InputResult::Eof);
            }
            Ok(_) => {
                let _ = tx.send(InputResult::Response(line));
            }
            Err(_) => {
                let _ = tx.send(InputResult::Interrupted);
            }
        }
    });

    match rx.recv_timeout(Duration::from_secs(timeout_secs)) {
        Ok(result) => result,
        Err(mpsc::RecvTimeoutError::Timeout) => InputResult::Timeout,
        Err(_) => InputResult::Eof,
    }
}

// ── helpers ─────────────────────────────────────────────────────────────

/// Detect a non-interactive terminal (piped stdin, no TTY).
fn is_noninteractive() -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::io::AsRawFd;
        let fd = io::stdin().as_raw_fd();
        // SAFETY: isatty only reads the fd.
        unsafe { libc::isatty(fd) == 0 }
    }
    #[cfg(not(unix))]
    {
        // Conservative: assume interactive on non-Unix.
        false
    }
}

/// Remove the `.disabled` file with fail-open error handling.
fn remove_disabled_file_safe(disabled_file: &Path, context: Option<&str>) {
    let suffix = context.map_or(String::new(), |c| format!(" {c}"));
    match std::fs::remove_file(disabled_file) {
        Ok(()) => {
            println!("\n✓ Power-Steering re-enabled{suffix}.\n");
        }
        Err(e) if !disabled_file.exists() => {
            // Already removed (concurrent access).
            println!("\n✓ Power-Steering re-enabled{suffix}.\n");
            let _ = e; // suppress unused warning
        }
        Err(e) => {
            tracing::warn!(error = %e, "could not remove .disabled file");
        }
    }
}

#[cfg(test)]
#[path = "tests/power_steering_tests.rs"]
mod tests;
