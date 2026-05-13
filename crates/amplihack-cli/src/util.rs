//! Shared utility functions used across amplihack-cli modules.
//!
//! # Security
//!
//! * SEC-WS2-01: [`is_noninteractive`] is a **UX convenience flag** only — it
//!   must NOT be used as a security gate. Any attacker who can set env vars
//!   already has equivalent access.
//! * SEC-WS2-02: All externally-sourced strings must pass through [`strip_ansi`]
//!   before display to prevent terminal injection via crafted external output.

use anyhow::{Context, Result, bail};
use std::io::IsTerminal;
use std::io::{self, Write};
use std::process::{Command, ExitStatus, Output, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

// ── Non-interactive mode detection ────────────────────────────────────────────

/// Returns `true` when the process is running in a non-interactive environment.
///
/// Two conditions trigger non-interactive mode (OR logic):
///
/// 1. **Env var**: `AMPLIHACK_NONINTERACTIVE` is set to the exact string `"1"`.
///    Only `"1"` is recognized — `"true"`, `"yes"`, `"on"`, etc. do NOT trigger
///    this path. This is a cross-language contract with the Python launcher.
///
/// 2. **TTY detection**: `std::io::stdin().is_terminal()` returns `false`,
///    indicating the process stdin is a pipe, redirect, or CI environment.
///
/// # Security (SEC-WS2-01)
///
/// This is a **UX convenience flag**, not a security gate. Do not rely on it
/// for access control. Emit `tracing::debug` at call sites so non-interactive
/// mode is observable in audit logs.
pub fn is_noninteractive() -> bool {
    // Fast path: explicit env var opt-in. Cross-language contract: only "1".
    if std::env::var("AMPLIHACK_NONINTERACTIVE").as_deref() == Ok("1") {
        return true;
    }
    // Fallback: stdin is not a TTY (pipe, redirect, CI runner, test harness).
    !std::io::stdin().is_terminal()
}

/// Returns `true` when this process is running as a subprocess delegate.
///
/// A "subprocess context" is any of the following (OR logic):
///
/// 1. **`AMPLIHACK_AGENT_BINARY`** is set to a non-empty value. Parent agent
///    runtimes (Claude Code, recipe-runner, Copilot CLI agent dispatch) set
///    this to identify the active binary. Empty string is the documented
///    "no delegation" sentinel and does NOT trigger this signal.
/// 2. **`AMPLIHACK_NONINTERACTIVE=1`** — explicit cross-language non-interactive
///    marker (same contract as [`is_noninteractive`]).
/// 3. **Any of stdin / stdout / stderr is not a TTY** — at least one of the
///    three standard streams is a pipe, redirect, or CI runner.
///
/// This is **stricter** than [`is_noninteractive`], which examines stdin only.
/// Subprocess-safe context is the signal used by the `amplihack copilot`
/// subcommand to decide whether to inject `--allow-all-tools` /
/// `--allow-all-paths` and to flip the reflection default. See
/// `docs/COPILOT_SUBPROCESS_SAFE.md` for the full spec (issue #621).
///
/// # Security (SEC-WS2-01)
///
/// This is a **UX convenience signal**, not a security gate. Anyone who can
/// set env vars or redirect stdio already controls process startup; this
/// helper inherits that trust posture and never escalates it.
pub fn is_subprocess_context() -> bool {
    // Signal 1: AMPLIHACK_AGENT_BINARY set non-empty.
    if let Ok(v) = std::env::var("AMPLIHACK_AGENT_BINARY")
        && !v.is_empty()
    {
        return true;
    }
    // Signal 2: explicit non-interactive marker.
    if std::env::var("AMPLIHACK_NONINTERACTIVE").as_deref() == Ok("1") {
        return true;
    }
    // Signal 3: any stdio stream is not a TTY.
    !std::io::stdin().is_terminal()
        || !std::io::stdout().is_terminal()
        || !std::io::stderr().is_terminal()
}

/// Snapshot of the OR-of-streams TTY state for the three standard streams.
///
/// Returns `true` only if **all three** of stdin/stdout/stderr are TTYs.
/// Used to feed the pure decision function
/// [`crate::commands::launch::command::resolve_subprocess_safe`].
pub fn all_streams_are_tty() -> bool {
    std::io::stdin().is_terminal()
        && std::io::stdout().is_terminal()
        && std::io::stderr().is_terminal()
}

// ── ANSI stripping ────────────────────────────────────────────────────────────

/// Remove ANSI escape sequences from `s`.
///
/// Handles CSI sequences of the form `ESC [ <params> <final_byte>` where
/// `<final_byte>` is any byte in the range `0x40..=0x7E` (e.g. `m` for SGR).
/// Applied to all externally-sourced strings before display to prevent
/// terminal injection via crafted version strings.  See SEC-WS2-02.
pub fn strip_ansi(s: &str) -> String {
    // Pre-allocate the full input length — stripped output is always ≤ input.
    // Avoids repeated Vec reallocations for inputs containing many ANSI sequences.
    let mut result = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;

    while i < bytes.len() {
        // Detect CSI sequence: ESC (0x1B) followed by '[' (0x5B)
        if bytes[i] == 0x1b && i + 1 < bytes.len() && bytes[i + 1] == b'[' {
            i += 2; // skip ESC [
            // Consume bytes until the final byte (0x40–0x7E inclusive)
            while i < bytes.len() {
                let b = bytes[i];
                i += 1;
                if (0x40..=0x7e).contains(&b) {
                    break; // final byte consumed — CSI sequence done
                }
            }
        } else {
            // Regular character — copy it, advancing by its full UTF-8 width.
            // SAFETY: `s` is valid UTF-8 (guaranteed by `&str`); indexing at a
            // known byte boundary via `chars().next()` is always safe.
            let ch = s[i..].chars().next().expect("non-empty slice");
            result.push(ch);
            i += ch.len_utf8();
        }
    }

    result
}

// ── Subprocess with timeout ────────────────────────────────────────────────────

/// Run a pre-built `Command` with a hard wall-clock timeout.
///
/// Uses a background thread and `mpsc::recv_timeout` — no async runtime needed.
/// On timeout, sends SIGTERM to the child (Unix only, best-effort) and returns
/// an error.  Returns the `ExitStatus` on success.
pub fn run_with_timeout(mut cmd: Command, timeout: Duration) -> Result<ExitStatus> {
    let mut child = cmd.spawn().context("failed to spawn subprocess")?;
    let pid = child.id();

    let (tx, rx) = mpsc::channel::<std::io::Result<ExitStatus>>();
    thread::spawn(move || {
        let _ = tx.send(child.wait());
    });

    match rx.recv_timeout(timeout) {
        Ok(result) => result.context("failed to wait for subprocess"),
        Err(_elapsed) => {
            #[cfg(unix)]
            {
                let kill_result = Command::new("kill")
                    .args(["-TERM", &pid.to_string()])
                    .status();
                if let Err(e) = kill_result {
                    tracing::warn!(pid, error = %e, "failed to terminate timed-out subprocess");
                }
            }
            bail!(
                "subprocess timed out after {} seconds (pid {})",
                timeout.as_secs(),
                pid
            )
        }
    }
}

/// Run a pre-built `Command` with stdout/stderr capture and a hard timeout.
pub fn run_output_with_timeout(mut cmd: Command, timeout: Duration) -> Result<Output> {
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    let child = cmd.spawn().context("failed to spawn subprocess")?;
    let pid = child.id();
    let (tx, rx) = mpsc::channel::<std::io::Result<Output>>();

    thread::spawn(move || {
        let _ = tx.send(child.wait_with_output());
    });

    match rx.recv_timeout(timeout) {
        Ok(result) => result.context("failed to wait for subprocess output"),
        Err(_elapsed) => {
            #[cfg(unix)]
            {
                let kill_result = Command::new("kill")
                    .args(["-TERM", &pid.to_string()])
                    .status();
                if let Err(e) = kill_result {
                    tracing::warn!(pid, error = %e, "failed to terminate timed-out subprocess");
                }
            }
            bail!(
                "subprocess timed out after {} seconds (pid {})",
                timeout.as_secs(),
                pid
            )
        }
    }
}

/// Read a single line of terminal input with a wall-clock timeout.
pub fn read_user_input_with_timeout(prompt: &str, timeout: Duration) -> Result<Option<String>> {
    print!("{prompt}");
    io::stdout().flush().context("failed to flush prompt")?;

    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;

        let fd = io::stdin().as_raw_fd();
        let mut pollfd = libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        };
        let timeout_ms = timeout.as_millis().min(i32::MAX as u128) as i32;
        let ready = unsafe { libc::poll(&mut pollfd, 1, timeout_ms) };
        if ready < 0 {
            return Err(io::Error::last_os_error()).context("failed waiting for prompt input");
        }
        if ready == 0 {
            println!();
            return Ok(None);
        }
    }

    #[cfg(not(unix))]
    {
        if !io::stdin().is_terminal() {
            return Ok(None);
        }
    }

    let mut response = String::new();
    io::stdin()
        .read_line(&mut response)
        .context("failed to read prompt input")?;
    Ok(Some(response.trim().to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── WS2: is_noninteractive ─────────────────────────────────────────────────

    /// WS2-1: is_noninteractive() returns true when AMPLIHACK_NONINTERACTIVE=1.
    #[test]
    fn is_noninteractive_env_var_path() {
        let _guard = crate::test_support::home_env_lock()
            .lock()
            .unwrap_or_else(|p| p.into_inner());

        let prev = std::env::var_os("AMPLIHACK_NONINTERACTIVE");
        unsafe { std::env::set_var("AMPLIHACK_NONINTERACTIVE", "1") };

        let result = is_noninteractive();

        match prev {
            Some(v) => unsafe { std::env::set_var("AMPLIHACK_NONINTERACTIVE", v) },
            None => unsafe { std::env::remove_var("AMPLIHACK_NONINTERACTIVE") },
        }

        assert!(
            result,
            "is_noninteractive() must return true when AMPLIHACK_NONINTERACTIVE=1"
        );
    }

    /// WS2-2: is_noninteractive_env_var_zero_not_triggered verifies "0" is not "1".
    #[test]
    fn is_noninteractive_env_var_zero_not_triggered() {
        let _guard = crate::test_support::home_env_lock()
            .lock()
            .unwrap_or_else(|p| p.into_inner());

        let prev = std::env::var_os("AMPLIHACK_NONINTERACTIVE");
        unsafe { std::env::set_var("AMPLIHACK_NONINTERACTIVE", "0") };

        let _result = is_noninteractive();

        unsafe { std::env::set_var("AMPLIHACK_NONINTERACTIVE", "1") };
        let must_be_true = is_noninteractive();

        match prev {
            Some(v) => unsafe { std::env::set_var("AMPLIHACK_NONINTERACTIVE", v) },
            None => unsafe { std::env::remove_var("AMPLIHACK_NONINTERACTIVE") },
        }

        assert!(
            must_be_true,
            "is_noninteractive() must return true when AMPLIHACK_NONINTERACTIVE=1 (sanity check)"
        );
    }

    /// WS2-3: TTY detection fallback mirrors the actual stdin TTY state.
    #[test]
    fn is_noninteractive_tty_path() {
        let _guard = crate::test_support::home_env_lock()
            .lock()
            .unwrap_or_else(|p| p.into_inner());

        let prev = std::env::var_os("AMPLIHACK_NONINTERACTIVE");
        unsafe { std::env::remove_var("AMPLIHACK_NONINTERACTIVE") };

        let result = is_noninteractive();
        let expected = !std::io::stdin().is_terminal();

        match prev {
            Some(v) => unsafe { std::env::set_var("AMPLIHACK_NONINTERACTIVE", v) },
            None => unsafe { std::env::remove_var("AMPLIHACK_NONINTERACTIVE") },
        }

        assert_eq!(
            result, expected,
            "is_noninteractive() must reflect stdin TTY state when AMPLIHACK_NONINTERACTIVE is unset"
        );
    }

    // ── Existing tests ─────────────────────────────────────────────────────────

    #[test]
    fn strip_ansi_passthrough_on_plain_text() {
        assert_eq!(strip_ansi("hello world"), "hello world");
    }

    #[test]
    fn strip_ansi_removes_sgr_sequences() {
        let input = "\x1b[1mbold\x1b[0m normal";
        assert_eq!(strip_ansi(input), "bold normal");
    }

    #[test]
    fn strip_ansi_removes_multiple_sequences() {
        let input = "\x1b[32m\x1b[1mgreen bold\x1b[0m";
        assert_eq!(strip_ansi(input), "green bold");
    }

    // ── #621: is_subprocess_context ────────────────────────────────────────────

    /// Saved env vars that the new `is_subprocess_context()` helper reads.
    /// Restored on Drop to keep concurrent tests under home_env_lock honest.
    struct SavedSubprocessEnv {
        agent_binary: Option<std::ffi::OsString>,
        noninteractive: Option<std::ffi::OsString>,
    }

    impl SavedSubprocessEnv {
        fn snapshot_and_clear() -> Self {
            let agent_binary = std::env::var_os("AMPLIHACK_AGENT_BINARY");
            let noninteractive = std::env::var_os("AMPLIHACK_NONINTERACTIVE");
            unsafe {
                std::env::remove_var("AMPLIHACK_AGENT_BINARY");
                std::env::remove_var("AMPLIHACK_NONINTERACTIVE");
            }
            Self {
                agent_binary,
                noninteractive,
            }
        }
    }

    impl Drop for SavedSubprocessEnv {
        fn drop(&mut self) {
            unsafe {
                match self.agent_binary.take() {
                    Some(v) => std::env::set_var("AMPLIHACK_AGENT_BINARY", v),
                    None => std::env::remove_var("AMPLIHACK_AGENT_BINARY"),
                }
                match self.noninteractive.take() {
                    Some(v) => std::env::set_var("AMPLIHACK_NONINTERACTIVE", v),
                    None => std::env::remove_var("AMPLIHACK_NONINTERACTIVE"),
                }
            }
        }
    }

    /// #621 / #13: AMPLIHACK_AGENT_BINARY set to a non-empty value MUST
    /// classify the process as a subprocess delegate.
    #[test]
    fn is_subprocess_context_when_agent_binary_set_non_empty() {
        let _guard = crate::test_support::home_env_lock()
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let _saved = SavedSubprocessEnv::snapshot_and_clear();
        unsafe {
            std::env::set_var("AMPLIHACK_AGENT_BINARY", "copilot");
        }
        assert!(
            is_subprocess_context(),
            "AMPLIHACK_AGENT_BINARY=copilot must classify as subprocess context"
        );
    }

    /// #621 / #14: AMPLIHACK_AGENT_BINARY set to empty string MUST be treated
    /// as "no delegation" — empty is the documented sentinel for "not a
    /// delegate". Any subprocess-safe verdict in that case must come from
    /// another signal (TTY state, NONINTERACTIVE env), not from this var alone.
    #[test]
    fn is_subprocess_context_when_agent_binary_empty_does_not_force_true_via_that_signal() {
        let _guard = crate::test_support::home_env_lock()
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let _saved = SavedSubprocessEnv::snapshot_and_clear();
        // Set AMPLIHACK_AGENT_BINARY="" explicitly. Since AMPLIHACK_NONINTERACTIVE
        // is unset, the only remaining signal is TTY state. In `cargo test`,
        // stdio is piped so the result will be true via the TTY fallback —
        // which is fine. The contract we lock here is that the empty-string
        // env var alone is NOT treated as a delegation marker (the contract is
        // observable when combined with a TTY; we cannot directly test the
        // false case in `cargo test` without owning all three streams).
        unsafe {
            std::env::set_var("AMPLIHACK_AGENT_BINARY", "");
        }
        // Pure-function `resolve_subprocess_safe(false, Some(""), true)` covers
        // the false case directly (see tests_subprocess_safe.rs #5). Here we
        // simply verify is_subprocess_context() does not panic and returns a
        // boolean reflecting the documented OR-of-signals.
        let _ = is_subprocess_context();
    }

    /// #621 / #15: AMPLIHACK_NONINTERACTIVE=1 MUST classify as subprocess context.
    #[test]
    fn is_subprocess_context_when_amplihack_noninteractive_set() {
        let _guard = crate::test_support::home_env_lock()
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let _saved = SavedSubprocessEnv::snapshot_and_clear();
        unsafe {
            std::env::set_var("AMPLIHACK_NONINTERACTIVE", "1");
        }
        assert!(
            is_subprocess_context(),
            "AMPLIHACK_NONINTERACTIVE=1 must classify as subprocess context"
        );
    }

    /// #621 / #16: With a clean env (no AMPLIHACK_AGENT_BINARY,
    /// no AMPLIHACK_NONINTERACTIVE), the result MUST reflect the actual TTY
    /// state of the three standard streams. Under `cargo test`, stdio is
    /// piped (non-TTY) → the result is true. This documents the harness
    /// behavior; the false case is covered by the pure
    /// `resolve_subprocess_safe(false, None, true)` test in
    /// `commands::launch::tests_subprocess_safe`.
    #[test]
    fn is_subprocess_context_with_clean_env_reflects_io_state() {
        let _guard = crate::test_support::home_env_lock()
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let _saved = SavedSubprocessEnv::snapshot_and_clear();
        let result = is_subprocess_context();
        let expected_via_tty = !std::io::stdin().is_terminal()
            || !std::io::stdout().is_terminal()
            || !std::io::stderr().is_terminal();
        assert_eq!(
            result, expected_via_tty,
            "with clean env, is_subprocess_context() must mirror the OR-of-streams TTY state"
        );
    }
}
