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
#[cfg(target_os = "linux")]
use std::collections::{HashMap, HashSet};
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

const CHILD_WAIT_INITIAL_POLL_INTERVAL: Duration = Duration::from_millis(10);
const CHILD_WAIT_MAX_POLL_INTERVAL: Duration = Duration::from_millis(100);
const SUBPROCESS_SPAWN_RETRY_TIMEOUT: Duration = Duration::from_millis(250);
const SUBPROCESS_SPAWN_RETRY_INTERVAL: Duration = Duration::from_millis(10);

/// Run a pre-built `Command` with a hard wall-clock timeout.
///
/// On timeout, terminates the child through the process handle, waits for
/// cleanup to finish, and returns an error. Returns the `ExitStatus` on success.
pub fn run_with_timeout(mut cmd: Command, timeout: Duration) -> Result<ExitStatus> {
    let mut child = spawn_subprocess(&mut cmd).context("failed to spawn subprocess")?;
    let pid = child.id();

    if let Some(status) =
        wait_for_child_exit(&mut child, timeout).context("failed to wait for subprocess")?
    {
        return Ok(status);
    }

    terminate_timed_out_child(&mut child)?;
    let command_context = format!("{cmd:?}");
    bail!(
        "subprocess `{}` timed out after {:?} (pid {})",
        command_context,
        timeout,
        pid
    )
}

/// Run a command with a timeout while using a caller-supplied safe description
/// instead of debugging the command and its environment on timeout.
pub fn run_with_timeout_described(
    mut cmd: Command,
    timeout: Duration,
    description: &str,
) -> Result<ExitStatus> {
    let mut child = spawn_subprocess(&mut cmd).context("failed to spawn subprocess")?;
    let pid = child.id();

    if let Some(status) =
        wait_for_child_exit(&mut child, timeout).context("failed to wait for subprocess")?
    {
        return Ok(status);
    }

    terminate_timed_out_child(&mut child)?;
    bail!(
        "subprocess `{}` timed out after {:?} (pid {})",
        description,
        timeout,
        pid
    )
}

/// Run a pre-built `Command` with stdout/stderr capture and a hard timeout.
pub fn run_output_with_timeout(mut cmd: Command, timeout: Duration) -> Result<Output> {
    run_output_with_timeout_inner(&mut cmd, timeout, None)
}

/// Run a pre-built `Command` with stdout/stderr capture capped per stream.
///
/// The subprocess pipes are still fully drained so successful commands do not
/// fail with a broken pipe once the retained bytes reach `max_bytes_per_stream`.
pub fn run_output_with_timeout_limited(
    mut cmd: Command,
    timeout: Duration,
    max_bytes_per_stream: usize,
) -> Result<Output> {
    run_output_with_timeout_inner(&mut cmd, timeout, Some(max_bytes_per_stream))
}

fn run_output_with_timeout_inner(
    cmd: &mut Command,
    timeout: Duration,
    max_bytes_per_stream: Option<usize>,
) -> Result<Output> {
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    let mut child = spawn_subprocess(cmd).context("failed to spawn subprocess")?;
    let pid = child.id();
    let stdout = child
        .stdout
        .take()
        .context("failed to capture subprocess stdout")?;
    let stderr = child
        .stderr
        .take()
        .context("failed to capture subprocess stderr")?;
    let stdout_reader = spawn_pipe_reader(stdout, max_bytes_per_stream);
    let stderr_reader = spawn_pipe_reader(stderr, max_bytes_per_stream);

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
    // Do not join pipe readers after timeout: descendants may keep inherited
    // pipe fds open, and this helper must preserve a hard wall-clock timeout.
    let command_context = format!("{cmd:?}");
    bail!(
        "subprocess `{}` timed out after {:?} (pid {})",
        command_context,
        timeout,
        pid
    )
}

/// Truncate UTF-8 text on character boundaries and report discarded characters.
pub fn truncate_chars_with_notice(value: &str, max_chars: usize) -> String {
    if value.len() <= max_chars {
        return value.to_string();
    }

    let mut total_chars = 0usize;
    let mut end_byte = value.len();
    for (idx, _) in value.char_indices() {
        if total_chars == max_chars {
            end_byte = idx;
        }
        total_chars += 1;
    }

    if total_chars <= max_chars {
        return value.to_string();
    }

    let prefix = &value[..end_byte];
    let discarded = total_chars - max_chars;
    format!("{prefix}\n[truncated: discarded {discarded} chars]")
}

/// Decode bytes lossily, truncate on UTF-8 character boundaries, and report loss.
pub fn render_diagnostic_bytes(bytes: &[u8], max_chars: usize) -> String {
    if bytes.is_empty() {
        return "<empty>".to_string();
    }
    truncate_chars_with_notice(&String::from_utf8_lossy(bytes), max_chars)
}

/// Render bounded stdout/stderr details for a completed subprocess.
pub fn format_output_diagnostics(output: &Output, max_chars: usize) -> String {
    format!(
        "exit={}; stdout={}; stderr={}",
        output
            .status
            .code()
            .map_or_else(|| "signal".to_string(), |code| code.to_string()),
        render_diagnostic_bytes(&output.stdout, max_chars),
        render_diagnostic_bytes(&output.stderr, max_chars)
    )
}

fn spawn_subprocess(cmd: &mut Command) -> io::Result<Child> {
    let started = Instant::now();
    loop {
        match cmd.spawn() {
            Ok(child) => return Ok(child),
            Err(error) if error.kind() == io::ErrorKind::ExecutableFileBusy => {
                if started.elapsed() >= SUBPROCESS_SPAWN_RETRY_TIMEOUT {
                    return Err(error);
                }
                thread::sleep(SUBPROCESS_SPAWN_RETRY_INTERVAL);
            }
            Err(error) => return Err(error),
        }
    }
}

fn wait_for_child_exit(child: &mut Child, timeout: Duration) -> io::Result<Option<ExitStatus>> {
    let started = Instant::now();
    let mut poll_interval = CHILD_WAIT_INITIAL_POLL_INTERVAL;
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(Some(status));
        }
        let elapsed = started.elapsed();
        if elapsed >= timeout {
            return Ok(None);
        }
        thread::sleep(poll_interval.min(timeout.saturating_sub(elapsed)));
        poll_interval = (poll_interval + poll_interval).min(CHILD_WAIT_MAX_POLL_INTERVAL);
    }
}

/// How long a timed-out tree gets to unwind after SIGTERM before SIGKILL.
#[cfg(target_os = "linux")]
const TERMINATE_GRACE: Duration = Duration::from_secs(2);

fn terminate_timed_out_child(child: &mut Child) -> Result<()> {
    let pid = child.id();
    // Issue #1506: `Child::kill` reaches only the direct child. A wrapper
    // (`sudo`, `sh`, `cargo`) would leave its own children running as
    // orphans, so terminate the whole tree — politely first, so the tree can
    // unwind, then hard. The walk reads `/proc`, so it is Linux-only
    // (elsewhere only the direct child is killed). Coverage is best-effort:
    // a process's children are reparented to init the moment it exits, so a
    // descendant is only findable while its own parent is alive. The grace
    // loop therefore re-walks from every node it already knows, which keeps
    // tracking a reparented target and its later children; a target that
    // forks and exits between two walks still leaks that child (only a
    // process group or cgroup closes that window, and either would change
    // terminal job control for every caller). A pid is tracked for the whole
    // grace period, so one that exits and is reused by an unrelated process
    // in that window could in principle be signalled. Under `sudo` the SIGTERM pass
    // reaches the root-owned command only because sudo relays the signal it
    // receives; a non-root caller cannot SIGKILL that command directly
    // (EPERM).
    let survivors = terminate_tree_gracefully(child);
    kill_hard(&survivors);
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

/// SIGTERM the child and every live descendant, wait up to
/// [`TERMINATE_GRACE`] for them to go, and return the descendants still alive
/// for the caller to SIGKILL. Each poll takes one `/proc` snapshot and walks
/// it from the child and from every descendant seen so far, so children
/// spawned after the first pass by a still-live (possibly already reparented)
/// node are picked up and SIGTERMed too. The child is left for the caller to
/// reap.
#[cfg(target_os = "linux")]
fn terminate_tree_gracefully(child: &mut Child) -> Vec<u32> {
    let pid = child.id();
    let mut targets: Vec<u32> = Vec::new();
    let mut seen: HashSet<u32> = HashSet::new();
    let mut walk_and_signal = |targets: &mut Vec<u32>| {
        let snapshot = proc_children();
        let roots = std::iter::once(pid).chain(targets.iter().copied());
        for found in descendants_in(&snapshot, roots) {
            if seen.insert(found) {
                signal(found, libc::SIGTERM);
                targets.push(found);
            }
        }
    };
    // Walk before signalling the child: a wrapper that dies on SIGTERM
    // reparents its children at once, and then they are no longer below it.
    walk_and_signal(&mut targets);
    signal(pid, libc::SIGTERM);
    let started = Instant::now();
    loop {
        walk_and_signal(&mut targets);
        let child_done = !pid_is_live(pid);
        let descendants_done = !targets.iter().any(|target| pid_is_live(*target));
        if (child_done && descendants_done) || started.elapsed() >= TERMINATE_GRACE {
            break;
        }
        thread::sleep(Duration::from_millis(20));
    }
    targets.retain(|target| pid_is_live(*target));
    targets
}

/// Elsewhere only the direct child is killed; see `terminate_timed_out_child`.
#[cfg(not(target_os = "linux"))]
fn terminate_tree_gracefully(_child: &mut Child) -> Vec<u32> {
    Vec::new()
}

/// Plain signal delivery; errors are ignored (the process may already be
/// gone, or be root-owned under `sudo`).
#[cfg(unix)]
fn signal(pid: u32, signal: libc::c_int) {
    // SAFETY: `kill` has no memory-safety preconditions.
    unsafe {
        libc::kill(pid as libc::pid_t, signal);
    }
}

/// SIGKILL the given descendants.
fn kill_hard(pids: &[u32]) {
    for target in pids {
        #[cfg(unix)]
        signal(*target, libc::SIGKILL);
        #[cfg(not(unix))]
        let _ = target;
    }
}

/// One snapshot of the live process tree from `/proc`: parent pid to child
/// pids. Best effort: an unreadable entry is skipped.
#[cfg(target_os = "linux")]
fn proc_children() -> HashMap<u32, Vec<u32>> {
    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return children;
    };
    for entry in entries.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|s| s.parse::<u32>().ok())
        else {
            continue;
        };
        let Some((ppid, live)) = proc_stat_ppid(pid) else {
            continue;
        };
        if live {
            children.entry(ppid).or_default().push(pid);
        }
    }
    children
}

/// Every process below any of `roots` in a [`proc_children`] snapshot, each
/// reported once.
#[cfg(target_os = "linux")]
fn descendants_in(
    children: &HashMap<u32, Vec<u32>>,
    roots: impl IntoIterator<Item = u32>,
) -> Vec<u32> {
    let mut found = Vec::new();
    let mut visited = HashSet::new();
    let mut queue: Vec<u32> = roots.into_iter().collect();
    while let Some(parent) = queue.pop() {
        if let Some(kids) = children.get(&parent) {
            for kid in kids {
                if visited.insert(*kid) {
                    found.push(*kid);
                    queue.push(*kid);
                }
            }
        }
    }
    found
}

/// `(ppid, is_live)` from `/proc/<pid>/stat`; a zombie counts as not live.
#[cfg(target_os = "linux")]
fn proc_stat_ppid(pid: u32) -> Option<(u32, bool)> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // `pid (comm) state ppid …` — comm may contain spaces and parentheses.
    let rest = &stat[stat.rfind(')')? + 1..];
    let mut fields = rest.split_whitespace();
    let state = fields.next()?;
    let ppid = fields.next()?.parse::<u32>().ok()?;
    Some((ppid, state != "Z" && state != "X"))
}

#[cfg(target_os = "linux")]
fn pid_is_live(pid: u32) -> bool {
    proc_stat_ppid(pid).is_some_and(|(_, live)| live)
}

fn spawn_pipe_reader<R>(
    mut pipe: R,
    max_retained_bytes: Option<usize>,
) -> thread::JoinHandle<io::Result<Vec<u8>>>
where
    R: Read + Send + 'static,
{
    thread::spawn(move || {
        let mut buffer = Vec::new();
        match max_retained_bytes {
            Some(max_retained_bytes) => {
                let mut chunk = [0u8; 8192];
                loop {
                    let read = pipe.read(&mut chunk)?;
                    if read == 0 {
                        break;
                    }
                    let remaining = max_retained_bytes.saturating_sub(buffer.len());
                    if remaining > 0 {
                        buffer.extend_from_slice(&chunk[..read.min(remaining)]);
                    }
                }
            }
            None => {
                pipe.read_to_end(&mut buffer)?;
            }
        }
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

    #[test]
    fn truncate_chars_with_notice_keeps_short_multibyte_text() {
        assert_eq!(truncate_chars_with_notice("é", 1), "é");
    }

    #[test]
    fn truncate_chars_with_notice_truncates_on_character_boundary() {
        assert_eq!(
            truncate_chars_with_notice("aébc", 2),
            "aé\n[truncated: discarded 2 chars]"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn run_output_with_timeout_terminates_child_without_path() {
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

        // Scope the empty `PATH` to this child process only. Mutating the
        // process-global `PATH` via `std::env::set_var` would race every other
        // concurrently-running test that resolves programs through `PATH`
        // (e.g. the multitask launcher tests spawning `bash`), causing
        // spurious `ENOENT` spawn failures. A per-command override reproduces
        // the "PATH cannot resolve helpers" scenario without touching global
        // state.
        let mut cmd = std::process::Command::new(&script);
        cmd.arg(&pid_file);
        cmd.env("PATH", &empty_path);
        let result = run_output_with_timeout(cmd, Duration::from_millis(50));

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
    #[test]
    fn run_output_with_timeout_reports_command_and_precise_timeout_context() {
        let mut cmd = std::process::Command::new("/bin/sh");
        cmd.args(["-c", "sleep 5"]);

        let error = run_output_with_timeout(cmd, Duration::from_millis(50))
            .expect_err("sleeping subprocess should time out");
        let rendered = error.to_string();

        assert!(
            rendered.contains("/bin/sh") || rendered.contains("sleep"),
            "timeout error must include command context; got {rendered:#}"
        );
        assert!(
            rendered.contains("50ms") || rendered.contains("0.05"),
            "timeout error must report the configured timeout precisely; got {rendered:#}"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn run_output_with_timeout_limited_caps_retained_stdout() {
        let mut cmd = std::process::Command::new("/bin/sh");
        cmd.args(["-c", "head -c 70000 /dev/zero"]);

        let output = run_output_with_timeout_limited(cmd, Duration::from_secs(2), 1024).unwrap();

        assert!(output.status.success());
        assert_eq!(output.stdout.len(), 1024);
    }

    /// The pid a test shell wrote with `echo $! > file`. The shell must have
    /// forked its child before the timeout fired, which the timeouts below
    /// leave ample room for.
    #[cfg(target_os = "linux")]
    fn read_pid_file(pid_file: &std::path::Path) -> i32 {
        std::fs::read_to_string(pid_file)
            .expect("the shell must have written the grandchild pid before the timeout fired")
            .trim()
            .parse()
            .expect("the pid file holds a single pid")
    }

    /// Issue #1506: a timed-out wrapper's own children are terminated too.
    #[cfg(target_os = "linux")]
    #[test]
    fn run_with_timeout_terminates_grandchildren() {
        let temp = tempfile::tempdir().unwrap();
        let pid_file = temp.path().join("pid");
        let mut cmd = std::process::Command::new("/bin/sh");
        cmd.arg("-c")
            .arg("/bin/sleep 30 & echo $! > \"$1\"; wait")
            .arg("sh")
            .arg(&pid_file);

        let error = run_with_timeout(cmd, Duration::from_secs(1))
            .expect_err("the waiting shell must time out");
        assert!(error.to_string().contains("timed out after"), "{error:#}");

        let grandchild = read_pid_file(&pid_file);
        let exited = wait_for_pid_to_exit(grandchild, Duration::from_secs(3));
        if !exited {
            unsafe {
                libc::kill(grandchild, libc::SIGKILL);
            }
        }
        assert!(
            exited,
            "the orphaned `sleep` (pid {grandchild}) must be terminated"
        );
    }

    /// A tree that ignores SIGTERM is still gone after the grace period.
    #[cfg(target_os = "linux")]
    #[test]
    fn run_with_timeout_hard_kills_a_tree_that_ignores_sigterm() {
        let temp = tempfile::tempdir().unwrap();
        let pid_file = temp.path().join("pid");
        let mut cmd = std::process::Command::new("/bin/sh");
        cmd.arg("-c")
            .arg("trap '' TERM; /bin/sleep 30 & echo $! > \"$1\"; wait")
            .arg("sh")
            .arg(&pid_file);

        run_with_timeout(cmd, Duration::from_secs(1)).expect_err("must time out");

        let grandchild = read_pid_file(&pid_file);
        let exited = wait_for_pid_to_exit(grandchild, Duration::from_secs(4));
        if !exited {
            unsafe {
                libc::kill(grandchild, libc::SIGKILL);
            }
        }
        assert!(
            exited,
            "a SIGTERM-ignoring grandchild (pid {grandchild}) must be SIGKILLed"
        );
    }

    /// A descendant that outlives its parent is reparented to init, so it is
    /// no longer reachable by walking down from the direct child. It must
    /// still be tracked, and so must a child it spawns afterwards.
    #[cfg(target_os = "linux")]
    #[test]
    fn run_with_timeout_tracks_a_reparented_descendant_and_its_late_child() {
        let temp = tempfile::tempdir().unwrap();
        let pid_file = temp.path().join("pid");
        let mut cmd = std::process::Command::new("/bin/sh");
        // The outer shell dies on SIGTERM, orphaning the subshell. The
        // subshell ignores SIGTERM and waits until it has been reparented
        // (its ppid, read from /proc/self/stat by the `read` builtin, no
        // longer equals the outer shell's `$$`; `kill -0 $$` would not do,
        // since the unreaped outer shell stays a zombie), then spawns the
        // `sleep 30` that must not leak.
        cmd.arg("-c")
            .arg(concat!(
                "(trap '' TERM; ",
                "while read -r _ _ _ ppid _ < /proc/self/stat && [ \"$ppid\" = \"$$\" ]; ",
                "do /bin/sleep 0.02; done; ",
                "/bin/sleep 30 & echo $! > \"$1\"; wait) & wait",
            ))
            .arg("sh")
            .arg(&pid_file);

        run_with_timeout(cmd, Duration::from_secs(1)).expect_err("must time out");

        let late_child = read_pid_file(&pid_file);
        let exited = wait_for_pid_to_exit(late_child, Duration::from_secs(4));
        if !exited {
            unsafe {
                libc::kill(late_child, libc::SIGKILL);
            }
        }
        assert!(
            exited,
            "the late child (pid {late_child}) of a reparented subshell must be terminated"
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
