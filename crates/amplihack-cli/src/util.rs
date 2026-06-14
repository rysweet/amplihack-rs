//! Shared utility functions used across amplihack-cli modules.
//!
//! # Security
//!
//! * SEC-WS2-01: [`is_noninteractive`] is a **UX convenience flag** only — it
//!   must NOT be used as a security gate. Any attacker who can set env vars
//!   already has equivalent access.
//! * SEC-WS2-02: All externally-sourced strings must pass through [`strip_ansi`]
//!   before display to prevent terminal injection via crafted external output.

use anyhow::{Context, Result, anyhow, bail};
use std::io::IsTerminal;
use std::io::{self, Read, Write};
use std::process::{Child, Command, ExitStatus, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

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

/// Returns `true` if **any** of stdin / stdout / stderr is **not** a TTY.
///
/// This is the OR-of-streams TTY snapshot used by the subprocess-safe
/// context detection in the `amplihack copilot` dispatch (issue #621).
/// It is the polarity-consistent counterpart to the OR-of-signals logic
/// in [`crate::commands::launch::command::resolve_subprocess_safe`] — both
/// return `true` when the relevant signal indicates a non-interactive /
/// subprocess context.
///
/// **Distinct from [`is_noninteractive`]**, which examines stdin only.
/// Subprocess-safe context (issue #621) requires the stricter all-streams
/// view because parent agents (Claude Code, recipe-runner, Copilot CLI
/// agent dispatch) typically pipe both stdout and stderr.
pub fn any_stream_is_non_tty() -> bool {
    !std::io::stdin().is_terminal()
        || !std::io::stdout().is_terminal()
        || !std::io::stderr().is_terminal()
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
/// On timeout, terminates the child through the process handle, waits for
/// cleanup to finish, and returns an error. Returns the `ExitStatus` on success.
pub fn run_with_timeout(mut cmd: Command, timeout: Duration) -> Result<ExitStatus> {
    let mut child = cmd.spawn().context("failed to spawn subprocess")?;
    let pid = child.id();

    if let Some(status) =
        wait_for_child_exit(&mut child, timeout).context("failed to wait for subprocess")?
    {
        return Ok(status);
    }

    terminate_timed_out_child(&mut child)?;
    bail!(
        "subprocess timed out after {} seconds (pid {})",
        timeout.as_secs(),
        pid
    )
}

/// Run a pre-built `Command` with stdout/stderr capture and a hard timeout.
pub fn run_output_with_timeout(mut cmd: Command, timeout: Duration) -> Result<Output> {
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    let mut child = cmd.spawn().context("failed to spawn subprocess")?;
    let pid = child.id();
    let stdout = child
        .stdout
        .take()
        .context("failed to capture subprocess stdout")?;
    let stderr = child
        .stderr
        .take()
        .context("failed to capture subprocess stderr")?;
    let stdout_reader = spawn_pipe_reader(stdout);
    let stderr_reader = spawn_pipe_reader(stderr);

    if let Some(status) =
        wait_for_child_exit(&mut child, timeout).context("failed to wait for subprocess output")?
    {
        let stdout = join_pipe_reader(stdout_reader, "stdout")?;
        let stderr = join_pipe_reader(stderr_reader, "stderr")?;
        return Ok(Output {
            status,
            stdout,
            stderr,
        });
    }

    terminate_timed_out_child(&mut child)?;
    let _ = join_pipe_reader(stdout_reader, "stdout")?;
    let _ = join_pipe_reader(stderr_reader, "stderr")?;
    bail!(
        "subprocess timed out after {} seconds (pid {})",
        timeout.as_secs(),
        pid
    )
}

fn wait_for_child_exit(child: &mut Child, timeout: Duration) -> io::Result<Option<ExitStatus>> {
    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(Some(status));
        }
        if started.elapsed() >= timeout {
            return Ok(None);
        }
        thread::sleep(Duration::from_millis(10).min(timeout.saturating_sub(started.elapsed())));
    }
}

fn terminate_timed_out_child(child: &mut Child) -> Result<()> {
    let pid = child.id();
    match child.kill() {
        Ok(()) => {}
        Err(kill_error) => match child.try_wait() {
            Ok(Some(_)) => {}
            Ok(None) => {
                return Err(kill_error).with_context(|| {
                    format!("failed to terminate timed-out subprocess pid {pid}")
                });
            }
            Err(wait_error) => {
                return Err(wait_error).with_context(|| {
                    format!("failed to inspect timed-out subprocess pid {pid} after kill failure")
                });
            }
        },
    }
    child
        .wait()
        .with_context(|| format!("failed to wait for timed-out subprocess pid {pid}"))?;
    Ok(())
}

fn spawn_pipe_reader<R>(mut pipe: R) -> thread::JoinHandle<io::Result<Vec<u8>>>
where
    R: Read + Send + 'static,
{
    thread::spawn(move || {
        let mut buffer = Vec::new();
        pipe.read_to_end(&mut buffer)?;
        Ok(buffer)
    })
}

fn join_pipe_reader(
    reader: thread::JoinHandle<io::Result<Vec<u8>>>,
    stream_name: &str,
) -> Result<Vec<u8>> {
    reader
        .join()
        .map_err(|_| anyhow!("subprocess {stream_name} reader thread panicked"))?
        .with_context(|| format!("failed to read subprocess {stream_name}"))
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

    #[cfg(target_os = "linux")]
    #[test]
    fn run_output_with_timeout_terminates_child_without_path() {
        let _guard = crate::test_support::home_env_lock()
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let empty_path = temp.path().join("empty-path");
        std::fs::create_dir(&empty_path).unwrap();
        let script = temp.path().join("hang");
        let pid_file = temp.path().join("pid");
        std::fs::write(
            &script,
            "#!/bin/sh\nprintf '%s' \"$$\" > \"$1\"\nexec /bin/sleep 5\n",
        )
        .unwrap();
        let mut perms = std::fs::metadata(&script).unwrap().permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
        std::fs::set_permissions(&script, perms).unwrap();

        let previous_path = std::env::var_os("PATH");
        unsafe { std::env::set_var("PATH", &empty_path) };

        let mut cmd = std::process::Command::new(&script);
        cmd.arg(&pid_file);
        let result = run_output_with_timeout(cmd, Duration::from_millis(50));

        match previous_path {
            Some(value) => unsafe { std::env::set_var("PATH", value) },
            None => unsafe { std::env::remove_var("PATH") },
        }

        let pid: i32 = std::fs::read_to_string(&pid_file).unwrap().parse().unwrap();
        let exited = wait_for_pid_to_exit(pid, Duration::from_millis(500));
        if !exited {
            unsafe {
                libc::kill(pid, libc::SIGKILL);
            }
            let _ = wait_for_pid_to_exit(pid, Duration::from_millis(500));
        }

        let error = result.expect_err("hanging subprocess should time out");
        assert!(
            error.to_string().contains("timed out after"),
            "timeout error must stay explicit; got {error:#}"
        );
        assert!(
            exited,
            "timed-out subprocess must be terminated even when PATH cannot resolve kill"
        );
    }

    #[cfg(target_os = "linux")]
    fn wait_for_pid_to_exit(pid: i32, timeout: Duration) -> bool {
        let started = std::time::Instant::now();
        let proc_path = std::path::PathBuf::from(format!("/proc/{pid}"));
        while proc_path.exists() {
            if started.elapsed() >= timeout {
                return false;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        true
    }
}
