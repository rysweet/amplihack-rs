//! Binary finder — locates tool binaries (claude, copilot, codex, amplifier) in PATH.
//!
//! Uses `which`-style lookup with version verification. No fallbacks:
//! if the binary isn't found, we error out.

use anyhow::{Result, bail};
use std::collections::HashSet;
use std::env;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use std::time::Instant;

/// Metadata about a discovered binary.
#[derive(Debug, Clone)]
pub struct BinaryInfo {
    /// Tool name (e.g., "claude").
    pub name: String,
    /// Absolute path to the binary.
    pub path: PathBuf,
    /// Version string if available.
    pub version: Option<String>,
}

/// Finds tool binaries on the system PATH.
pub struct BinaryFinder;

impl BinaryFinder {
    /// Find a tool binary by name.
    ///
    /// Search order:
    /// 1. `AMPLIHACK_{TOOL}_BINARY_PATH` env var (exact override)
    /// 2. `{TOOL}_BINARY_PATH` env var (Python parity, e.g. CLAUDE_BINARY_PATH)
    /// 3. PATH search for known binary names
    ///
    /// Errors if the binary is not found. No fallbacks.
    pub fn find(tool: &str) -> Result<BinaryInfo> {
        let tool_upper = tool.to_uppercase();

        // Check explicit override env var (amplihack-prefixed)
        let env_key = format!("AMPLIHACK_{tool_upper}_BINARY_PATH");
        if let Ok(explicit_path) = env::var(&env_key) {
            let path = PathBuf::from(&explicit_path);
            if path.exists() {
                let version = detect_version(&path);
                return Ok(BinaryInfo {
                    name: tool.to_string(),
                    path,
                    version,
                });
            }
            bail!("{env_key}={explicit_path} does not exist");
        }

        // Check tool-native env var (Python parity, e.g. CLAUDE_BINARY_PATH)
        let native_env_key = format!("{tool_upper}_BINARY_PATH");
        if let Ok(explicit_path) = env::var(&native_env_key) {
            let path = PathBuf::from(&explicit_path);
            if path.exists() {
                let version = detect_version(&path);
                return Ok(BinaryInfo {
                    name: tool.to_string(),
                    path,
                    version,
                });
            }
            bail!("{native_env_key}={explicit_path} does not exist");
        }

        // Search PATH for known binary names, then fall back to the
        // directories where `amplihack install_tool` actually writes
        // binaries. Without the fallback we re-install npm/cargo tools on
        // every launch when the user's shell PATH hasn't been updated yet
        // (e.g. a pre-existing tmux or ssh session whose PATH was captured
        // before `persist_path_hint` wrote to `.bashrc`).
        let candidates = binary_candidates(tool);
        let mut search_dirs = search_path_dirs();
        let fallback_dirs = install_fallback_dirs();
        for dir in &fallback_dirs {
            if !search_dirs.contains(dir) {
                search_dirs.push(dir.clone());
            }
        }

        for candidate in &candidates {
            for dir in &search_dirs {
                let full_path = dir.join(candidate);
                if full_path.is_file() && is_executable(&full_path) {
                    let version = detect_version(&full_path);
                    return Ok(BinaryInfo {
                        name: tool.to_string(),
                        path: full_path,
                        version,
                    });
                }
            }
        }

        bail!(
            "binary for '{tool}' not found in PATH or known install dirs (searched for: {})",
            candidates.join(", ")
        );
    }
}

/// Return candidate binary names for a tool.
pub(crate) fn binary_candidates(tool: &str) -> Vec<String> {
    match tool {
        "claude" => vec!["rustyclawd".to_string(), "claude".to_string()],
        "copilot" => vec!["copilot".to_string()],
        "codex" => vec!["codex".to_string()],
        "amplifier" => vec!["amplifier".to_string()],
        other => vec![other.to_string()],
    }
}

/// Collect PATH directories into a de-duplicated, ordered Vec.
///
/// F-S5 — relative entries are dropped, for the same reason
/// `launch_target::path_dirs` drops them. POSIX reads an **empty** `$PATH`
/// element as the current directory, and trailing or doubled colons are
/// ordinary in hand-edited shell profiles: `split_paths("/usr/bin:")` yields
/// `["/usr/bin", ""]`, and joining `""` with `claude` gives the bare relative
/// name that `detect_version` then hands to `execvp`, which resolves it from
/// wherever amplihack happens to be. `git clone <repo> && cd repo && amplihack
/// claude` is the whole exploit.
///
/// This is a second, separate funnel from `launch_target`'s — different module,
/// different callers (`bootstrap.rs` reaches this one) — so it needs its own
/// filter. `.` and `..` are the same hazard spelled out, so the test is
/// absoluteness rather than emptiness.
fn search_path_dirs() -> Vec<PathBuf> {
    let path_var = env::var("PATH").unwrap_or_default();
    let mut seen = HashSet::new();
    let mut dirs = Vec::new();

    for entry in env::split_paths(&path_var).filter(|dir| dir.is_absolute()) {
        if seen.insert(entry.clone()) {
            dirs.push(entry);
        }
    }

    dirs
}

/// Known locations where `amplihack install_tool` writes binaries, regardless
/// of whether those directories are on the shell's `$PATH`.
///
/// Keeps binary discovery working for users whose `.bashrc` / `.zshrc` PATH
/// update hasn't been sourced yet (persistent tmux sessions, SSH sessions
/// started before the first amplihack install, Docker shells that inherit a
/// minimal PATH, etc.).
fn install_fallback_dirs() -> Vec<PathBuf> {
    let home = env::var_os("HOME").map(PathBuf::from);
    let mut dirs = Vec::new();
    if let Some(home) = home {
        // npm global prefix set by `install_npm_package`. One owner for the
        // spelling — see `launch_target::amplihack_prefix_bin`.
        dirs.push(crate::launch_target::amplihack_prefix_bin(&home));
        // `cargo install` default.
        dirs.push(home.join(".cargo").join("bin"));
        // `uv tool install` + legacy Python amplihack install target.
        dirs.push(home.join(".local").join("bin"));
    }
    dirs
}

/// Maximum number of characters to retain from a detected version string.
const MAX_VERSION_LEN: usize = 200;
const VERSION_DETECTION_TIMEOUT: Duration = Duration::from_millis(500);

/// Run `binary --version` and extract a version string.
fn detect_version(path: &Path) -> Option<String> {
    let mut cmd = Command::new(path);
    cmd.arg("--version");
    // SEC-3 — `path` is an arbitrary candidate from `$PATH` or `$HOME`, so its
    // stdout is attacker-influenced and its volume is attacker-chosen. This is
    // the one place in this module that probes untrusted binaries, and it used
    // to reach for an uncapped wrapper that passed `usize::MAX`, opting out of
    // `PROBE_CAPTURE_LIMIT` four lines from where that constant is documented
    // as "a hard cap on how many bytes a probed binary may push into memory".
    // A `--version` line is never 64 KiB, so the cap is behaviour-neutral in
    // every real case; the wrapper is gone so there is no uncapped sibling left
    // to reach for. The error is discarded here as it always was — capped or
    // timed out, an unreadable probe is not a version.
    let output =
        run_capped_output_with_timeout(cmd, VERSION_DETECTION_TIMEOUT, PROBE_CAPTURE_LIMIT)
            .ok()??;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let first_line = stdout.lines().next()?.trim();
    let version = strip_ansi(first_line);
    Some(truncate_chars_with_notice(&version, MAX_VERSION_LEN))
}

const CHILD_WAIT_INITIAL_POLL_INTERVAL: Duration = Duration::from_millis(10);
const CHILD_WAIT_MAX_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// How long to keep retrying a spawn that fails with `ExecutableFileBusy`
/// (ETXTBSY). This races the window right after a binary is written/installed
/// while its inode is still being closed by the writer.
const SUBPROCESS_SPAWN_RETRY_TIMEOUT: Duration = Duration::from_secs(2);
/// Delay between `ExecutableFileBusy` spawn retries.
const SUBPROCESS_SPAWN_RETRY_INTERVAL: Duration = Duration::from_millis(20);

/// Spawn a command, retrying briefly on `ExecutableFileBusy` (ETXTBSY).
///
/// A freshly written executable can still be held open for writing, so the
/// first `spawn` may fail with ETXTBSY; retry within a bounded window instead
/// of surfacing a spurious failure.
fn spawn_subprocess(cmd: &mut Command) -> std::io::Result<std::process::Child> {
    let started = Instant::now();
    loop {
        match cmd.spawn() {
            Ok(child) => return Ok(child),
            Err(error) if error.kind() == std::io::ErrorKind::ExecutableFileBusy => {
                if started.elapsed() >= SUBPROCESS_SPAWN_RETRY_TIMEOUT {
                    return Err(error);
                }
                thread::sleep(SUBPROCESS_SPAWN_RETRY_INTERVAL);
            }
            Err(error) => return Err(error),
        }
    }
}

/// Wait for `child`, waking early when the drain threads report EOF.
///
/// `drained` is held by both drain threads and by nobody else, so it
/// disconnects when both pipes hit EOF — which is the child closing its
/// stdio, i.e. exiting. Sleeping on that instead of on the next backoff tick
/// is worth roughly the poll interval on every probe: a `claude --version`
/// that takes 110 ms was detected at 150 ms by the backoff schedule
/// (10/20/40/80 ms), so a quarter of the measured cost of a resolution was
/// this function sleeping through an exit that had already happened.
///
/// Disconnect is a hint, not proof: a child can close its stdio and keep
/// running, and a grandchild can hold the pipes open past its parent's exit.
/// Both are handled — the `try_wait` at the top of the loop stays the
/// authority, and once the hint is spent the loop falls back to the same
/// bounded backoff it used before.
fn wait_for_child_exit(
    child: &mut std::process::Child,
    timeout: Duration,
    drained: &std::sync::mpsc::Receiver<std::convert::Infallible>,
) -> anyhow::Result<Option<std::process::ExitStatus>> {
    use std::sync::mpsc::RecvTimeoutError;

    let deadline = Instant::now() + timeout;
    let mut interval = CHILD_WAIT_INITIAL_POLL_INTERVAL;
    let mut pipes_open = true;
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(Some(status));
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Ok(None);
        }
        let nap = interval.min(remaining);
        if pipes_open {
            // Nothing is ever sent on this channel; only the disconnect
            // matters. `Ok` is unreachable — the payload type is uninhabited.
            match drained.recv_timeout(nap) {
                Err(RecvTimeoutError::Disconnected) => {
                    // Exit is imminent. Re-check now rather than sleeping.
                    pipes_open = false;
                    continue;
                }
                Err(RecvTimeoutError::Timeout) => {}
                Ok(never) => match never {},
            }
        } else {
            thread::sleep(nap);
        }
        interval = (interval * 2).min(CHILD_WAIT_MAX_POLL_INTERVAL);
    }
}

/// SEC-3: hard cap on how many bytes a probed binary may push into memory.
///
/// A version probe answers with one short line. Anything past this is either a
/// confused binary or a hostile one, and neither earns unbounded RAM. The
/// hardened runner in `amplihack-cli` is unreachable from here (the dependency
/// runs cli → launcher → utils), so the cap lives here rather than moving the
/// crate boundary.
pub(crate) const PROBE_CAPTURE_LIMIT: usize = 64 * 1024;

/// Read from `pipe` until EOF or `limit` bytes, whichever comes first, then
/// keep draining and discarding so the child never blocks on a full pipe.
///
/// Bytes land in `sink` as they arrive rather than in a local buffer the caller
/// can only see by joining. That is what makes the drain abandonable: when the
/// budget runs out the caller walks away from this thread and still gets
/// everything that had been read by then (SEC-4).
fn drain_pipe_capped(
    mut pipe: impl std::io::Read,
    limit: usize,
    sink: &Mutex<Vec<u8>>,
) -> std::io::Result<()> {
    let mut chunk = [0u8; 8192];
    loop {
        let read = pipe.read(&mut chunk)?;
        if read == 0 {
            return Ok(());
        }
        let mut buf = sink.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if buf.len() < limit {
            let take = read.min(limit - buf.len());
            buf.extend_from_slice(&chunk[..take]);
        }
        // Past the cap we keep reading and throwing the bytes away: closing the
        // pipe early would hand the child a SIGPIPE we did not intend.
    }
}

/// Run `cmd` with a timeout and a capped capture.
///
/// `Ok(None)` means the child exceeded `timeout` and was killed — distinct from
/// `Err`, which means the spawn itself failed. [`crate::launch_target`] needs
/// that distinction to tell `ProbeTimedOut` from `ProbeFailed`.
///
/// # The bound covers the drain, not just the wait (SEC-4)
///
/// Waiting for the child and then joining the reader threads unconditionally is
/// not a timeout, and measuring proved it: a probe of a shim that exits
/// immediately after backgrounding `sleep 60` returned in **60.0 s against a
/// 10 s budget**, because the backgrounded grandchild inherited stdout and the
/// drain thread never saw EOF. Point that at a daemon instead and it never
/// returns at all. `launch_target` probes every candidate on `$PATH` in order,
/// so that is exactly the threat SEC-4 names.
///
/// So the joins are bounded by whatever is left of `timeout`, and when it runs
/// out the readers are abandoned rather than waited on: the child's exit status
/// is already known and authoritative, and a version probe needs one semver
/// line, not a complete transcript. The detached threads exit on their own when
/// the pipe finally closes or when the process does.
pub(crate) fn run_capped_output_with_timeout(
    mut cmd: Command,
    timeout: Duration,
    limit: usize,
) -> anyhow::Result<Option<Output>> {
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    let started = Instant::now();
    let mut child = spawn_subprocess(&mut cmd)?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow::anyhow!("failed to capture subprocess stdout"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| anyhow::anyhow!("failed to capture subprocess stderr"))?;
    // Shared with the drain threads so the capture is readable without joining
    // them — see the timeout branch below.
    let stdout_buf = Arc::new(Mutex::new(Vec::new()));
    let stderr_buf = Arc::new(Mutex::new(Vec::new()));
    // Both drain threads own a clone of `eof_tx` and send nothing on it. The
    // clones drop when the threads finish, which disconnects `eof_rx` and wakes
    // the wait below the moment the child's pipes reach EOF.
    let (eof_tx, eof_rx) = std::sync::mpsc::channel::<std::convert::Infallible>();
    let stderr_tx = eof_tx.clone();
    let stdout_reader = {
        let sink = Arc::clone(&stdout_buf);
        thread::spawn(move || {
            let _eof_tx = eof_tx;
            drain_pipe_capped(stdout, limit, &sink)
        })
    };
    let stderr_reader = {
        let sink = Arc::clone(&stderr_buf);
        thread::spawn(move || {
            let _eof_tx = stderr_tx;
            drain_pipe_capped(stderr, limit, &sink)
        })
    };

    let Some(status) = wait_for_child_exit(&mut child, timeout, &eof_rx)? else {
        let _ = child.kill();
        let _ = child.wait();
        return Ok(None);
    };

    // `recv_timeout` on an already-disconnected channel returns `Disconnected`
    // immediately, so reusing `eof_rx` after `wait_for_child_exit` has already
    // consumed the hint is safe.
    let remaining = timeout.saturating_sub(started.elapsed());
    match eof_rx.recv_timeout(remaining) {
        // Both readers are done, so these joins do not block.
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
            stdout_reader
                .join()
                .map_err(|_| anyhow::anyhow!("stdout reader thread panicked"))??;
            stderr_reader
                .join()
                .map_err(|_| anyhow::anyhow!("stderr reader thread panicked"))??;
        }
        // Out of budget with a pipe still open: something other than the child
        // is holding it. Take what has been read so far and leave.
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
            tracing::debug!(
                ?timeout,
                "subprocess exited but its output pipes are still held open; \
                 returning a truncated capture rather than blocking"
            );
        }
        Ok(never) => match never {},
    }
    Ok(Some(Output {
        status,
        stdout: take_buffer(&stdout_buf),
        stderr: take_buffer(&stderr_buf),
    }))
}

/// Snapshot a drain buffer, leaving an empty one behind for a thread that may
/// still be writing to it.
fn take_buffer(buf: &Mutex<Vec<u8>>) -> Vec<u8> {
    std::mem::take(&mut *buf.lock().unwrap_or_else(|poisoned| poisoned.into_inner()))
}

/// Strip terminal escape sequences and control characters from `s`.
///
/// SEC-3: shared with [`crate::launch_target`], which needs it on both probe
/// stdout (attacker-controlled output from an arbitrary candidate binary) and
/// on rejected candidate *paths* (a planted filename can itself carry ESC).
/// There must not be a third copy of this in the crate.
///
/// Removed:
///
/// * **CSI** — `ESC [` … final byte in `0x40..=0x7e`.
/// * **String sequences** — `ESC ]` (OSC), `ESC P` (DCS), `ESC X` (SOS),
///   `ESC ^` (PM), `ESC _` (APC), each up to a `BEL` or an `ST` (`ESC \`).
///   OSC 52 writes the user's clipboard and OSC 0 rewrites the window title;
///   handling CSI alone let both straight through.
/// * **Two-byte escapes** — `ESC` plus one final byte, which covers `ESC c`
///   (RIS, a full terminal reset).
///
/// Every remaining C0 control (and `DEL`) becomes a **single space**, except
/// tab, which is kept. A space and not deletion, deliberately: deleting them
/// splices `"1.2.3\n4.5.6"` into `"1.2.34.5.6"`, which the semver regex in
/// [`crate::launch_target::extract_version`] happily reads as `1.2.34`. The
/// practical case is `LF` and `CR` — the rejection report renders
/// `"\n  {path}\n      {reason}\n"`, so a `$PATH` entry containing a newline
/// could otherwise forge extra rows and make a rejected candidate read as a
/// healthy one.
pub(crate) fn strip_ansi(s: &str) -> String {
    /// `ESC` introduces a sequence that runs to `BEL` or `ST` rather than to a
    /// single final byte: OSC, DCS, SOS, PM, APC.
    fn introduces_string_sequence(b: u8) -> bool {
        matches!(b, b']' | b'P' | b'X' | b'^' | b'_')
    }

    let mut result = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            0x1b => {
                i += 1;
                match bytes.get(i).copied() {
                    Some(b'[') => {
                        i += 1;
                        while i < bytes.len() {
                            let b = bytes[i];
                            i += 1;
                            if (0x40..=0x7e).contains(&b) {
                                break;
                            }
                        }
                    }
                    Some(b) if introduces_string_sequence(b) => {
                        i += 1;
                        while i < bytes.len() {
                            if bytes[i] == 0x07 {
                                i += 1;
                                break;
                            }
                            if bytes[i] == 0x1b && bytes.get(i + 1) == Some(&b'\\') {
                                i += 2;
                                break;
                            }
                            i += 1;
                        }
                    }
                    // ESC + one final byte, e.g. `ESC c`.
                    Some(_) => i += 1,
                    // A trailing ESC with nothing behind it.
                    None => {}
                }
            }
            b'\t' => {
                result.push('\t');
                i += 1;
            }
            b if b < 0x20 || b == 0x7f => {
                result.push(' ');
                i += 1;
            }
            _ => {
                let ch = s[i..].chars().next().expect("non-empty slice");
                result.push(ch);
                i += ch.len_utf8();
            }
        }
    }
    result
}

fn truncate_chars_with_notice(value: &str, max_chars: usize) -> String {
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

/// Check if a path is executable (Unix: has execute bit).
#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.metadata()
        .map(|m| m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binary_candidates_claude() {
        let candidates = binary_candidates("claude");
        assert!(candidates.contains(&"rustyclawd".to_string()));
        assert!(candidates.contains(&"claude".to_string()));
    }

    #[test]
    fn binary_candidates_unknown_tool() {
        let candidates = binary_candidates("newtool");
        assert_eq!(candidates, vec!["newtool"]);
    }

    #[test]
    fn search_path_dirs_deduplicates() {
        // PATH deduplication is deterministic
        let dirs = search_path_dirs();
        let unique: HashSet<_> = dirs.iter().collect();
        assert_eq!(dirs.len(), unique.len());
    }

    #[test]
    fn find_echo_binary() {
        // `echo` should be findable on any Unix system
        let result = BinaryFinder::find("echo");
        if let Ok(info) = result {
            assert!(info.path.exists());
            assert_eq!(info.name, "echo");
        }
        // If not found (unlikely), that's fine for a test
    }

    #[test]
    fn find_nonexistent_binary_errors() {
        let result = BinaryFinder::find("definitely_not_a_real_binary_xyz_123");
        assert!(result.is_err());
    }

    #[test]
    fn find_falls_back_to_npm_global_when_not_on_path() {
        // Simulate the hyenas2 scenario: copilot is installed at
        // ~/.npm-global/bin/copilot but the shell's $PATH was captured
        // before .bashrc was updated, so the shell doesn't include it.
        let _guard = crate::test_support::env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        let temp = tempfile::tempdir().unwrap();
        let fake_home = temp.path();
        let bin_dir = fake_home.join(".npm-global/bin");
        std::fs::create_dir_all(&bin_dir).unwrap();
        let fake_tool = bin_dir.join("needle-tool-xyz");
        std::fs::write(&fake_tool, "#!/bin/sh\necho needle\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&fake_tool, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        // Point HOME at the temp dir so install_fallback_dirs() resolves to
        // `fake_home/.npm-global/bin`, the only directory on this machine that
        // contains a file named `needle-tool-xyz`. Finding it there IS the
        // proof that the fallback ran: no $PATH entry can supply that name.
        //
        // Deliberately does NOT mutate $PATH. $PATH is process-global, and
        // libtest runs these tests on parallel threads alongside tests that
        // spawn `git` by bare name (artifact_guard, worktree). Clobbering it
        // here made those spawns fail with ENOENT — a real cross-test race,
        // not flakiness. The `env_lock` above only serialises env *mutators*;
        // the bare-name spawners are readers and never take it.
        let prev_home = env::var_os("HOME");
        // SAFETY: Serialized via env_lock above.
        unsafe {
            env::set_var("HOME", fake_home);
        }

        let result = BinaryFinder::find("needle-tool-xyz");

        // SAFETY: Still inside the env_lock critical section.
        unsafe {
            if let Some(v) = prev_home {
                env::set_var("HOME", v);
            } else {
                env::remove_var("HOME");
            }
        }

        let info = result.expect("fallback dir lookup should succeed");
        assert_eq!(info.path, fake_tool);
    }

    #[test]
    fn explicit_env_override() {
        // Set an explicit override pointing to /bin/echo
        let echo_path = if Path::new("/usr/bin/echo").exists() {
            "/usr/bin/echo"
        } else if Path::new("/bin/echo").exists() {
            "/bin/echo"
        } else {
            return; // Skip test if echo not found
        };

        // SAFETY: Test-only env var manipulation; test runner serializes tests by default.
        unsafe { env::set_var("AMPLIHACK_TESTTOOL_BINARY_PATH", echo_path) };
        let result = BinaryFinder::find("testtool");
        unsafe { env::remove_var("AMPLIHACK_TESTTOOL_BINARY_PATH") };

        let info = result.unwrap();
        assert_eq!(info.path, PathBuf::from(echo_path));
    }

    #[test]
    fn explicit_env_override_nonexistent_errors() {
        // SAFETY: Test-only env var manipulation; test runner serializes tests by default.
        unsafe {
            env::set_var(
                "AMPLIHACK_BADTOOL_BINARY_PATH",
                "/nonexistent/path/to/binary",
            );
        }
        let result = BinaryFinder::find("badtool");
        unsafe { env::remove_var("AMPLIHACK_BADTOOL_BINARY_PATH") };

        assert!(result.is_err());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn detect_version_truncates_without_panicking_on_utf8_boundary() {
        let temp = tempfile::tempdir().unwrap();
        let fake_tool = temp.path().join("version-tool");
        std::fs::write(
            &fake_tool,
            "#!/bin/sh\n\
             i=0\n\
             while [ \"$i\" -lt 80 ]; do printf '\\342\\202\\254'; i=$((i + 1)); done\n\
             printf '\\n'\n",
        )
        .unwrap();
        let mut perms = std::fs::metadata(&fake_tool).unwrap().permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
        std::fs::set_permissions(&fake_tool, perms).unwrap();

        let result = std::panic::catch_unwind(|| detect_version(&fake_tool));

        assert!(
            result.is_ok(),
            "detect_version must truncate on UTF-8 boundaries"
        );
        assert!(
            result.unwrap().is_some(),
            "valid lossy UTF-8 version output should still be detected"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn detect_version_is_bounded_for_hanging_binaries() {
        let temp = tempfile::tempdir().unwrap();
        let fake_tool = temp.path().join("hanging-version-tool");
        std::fs::write(&fake_tool, "#!/bin/sh\n/bin/sleep 2\nprintf 'v9.9.9\\n'\n").unwrap();
        let mut perms = std::fs::metadata(&fake_tool).unwrap().permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
        std::fs::set_permissions(&fake_tool, perms).unwrap();

        let started = std::time::Instant::now();
        let version = detect_version(&fake_tool);

        assert!(
            started.elapsed() < std::time::Duration::from_secs(1),
            "version detection must not wait for an unbounded child process"
        );
        assert!(
            version.is_none(),
            "timed-out version probes should not report a stale version"
        );
    }

    // ------------------------------------------------------------------
    // strip_ansi — SEC-3. Every case below reached the terminal verbatim
    // when this function recognized CSI and nothing else.
    // ------------------------------------------------------------------

    #[test]
    fn strip_ansi_removes_csi() {
        assert_eq!(strip_ansi("\x1b[31mred\x1b[0m"), "red");
    }

    #[test]
    fn strip_ansi_removes_osc_terminated_by_bel() {
        // OSC 52 writes the user's clipboard.
        assert_eq!(strip_ansi("\x1b]52;c;ZXZpbA==\x07x"), "x");
        // OSC 0 rewrites the window title.
        assert_eq!(strip_ansi("\x1b]0;pwned\x07x"), "x");
    }

    #[test]
    fn strip_ansi_removes_osc_terminated_by_st() {
        assert_eq!(strip_ansi("\x1b]0;pwned\x1b\\x"), "x");
    }

    #[test]
    fn strip_ansi_removes_a_two_byte_escape() {
        // RIS (ESC c) resets the whole terminal.
        assert_eq!(strip_ansi("\x1bcx"), "x");
    }

    #[test]
    fn strip_ansi_neutralizes_a_carriage_return_overwrite() {
        // "a\rSPOOFED" renders as "SPOOFED" on a real terminal: CR returns the
        // cursor to column 0 and the rest overwrites what was there.
        assert_eq!(strip_ansi("a\rSPOOFED"), "a SPOOFED");
    }

    #[test]
    fn strip_ansi_neutralizes_a_line_injection() {
        // A candidate path carrying newlines could otherwise forge extra rows
        // in the rejection report.
        assert_eq!(
            strip_ansi("/tmp/a\n  /usr/bin/claude\n      ok"),
            "/tmp/a   /usr/bin/claude       ok"
        );
    }

    #[test]
    fn strip_ansi_replaces_controls_with_a_space_rather_than_deleting_them() {
        // Deleting would splice these into "1.2.34.5.6", and the semver regex
        // in launch_target::extract_version reads that as "1.2.34" — a version
        // that was never printed, which would drive a spurious "upgrade".
        assert_eq!(strip_ansi("1.2.3\n4.5.6"), "1.2.3 4.5.6");
    }

    #[test]
    fn strip_ansi_keeps_tabs_and_ordinary_text() {
        assert_eq!(strip_ansi("a\tb"), "a\tb");
        assert_eq!(strip_ansi("hello world"), "hello world");
        assert_eq!(strip_ansi("héllo ✓"), "héllo ✓");
    }

    #[test]
    fn strip_ansi_survives_a_truncated_escape() {
        assert_eq!(strip_ansi("x\x1b"), "x");
        assert_eq!(strip_ansi("x\x1b["), "x");
        assert_eq!(strip_ansi("x\x1b]0;unterminated"), "x");
    }

    // ------------------------------------------------------------------
    // SEC-4: the timeout bounds the drain, not just the wait
    // ------------------------------------------------------------------

    #[cfg(target_os = "linux")]
    #[test]
    fn a_grandchild_holding_the_pipe_cannot_outlive_the_timeout() {
        // Measured before the fix: 60.0 s against a 10 s budget. The shim
        // exits immediately, but the backgrounded `sleep` inherits its stdout,
        // so the drain thread never sees EOF and the unconditional join blocked
        // for as long as the grandchild lived. Point it at a daemon instead and
        // it never returns at all.
        let temp = tempfile::tempdir().unwrap();
        let shim = temp.path().join("lingering");
        std::fs::write(
            &shim,
            "#!/bin/sh\n/bin/sleep 30 &\nprintf '1.2.3\\n'\nexit 0\n",
        )
        .unwrap();
        let mut perms = std::fs::metadata(&shim).unwrap().permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
        std::fs::set_permissions(&shim, perms).unwrap();

        let timeout = Duration::from_secs(2);
        let started = Instant::now();
        let output = run_capped_output_with_timeout(Command::new(&shim), timeout, 4096)
            .expect("the spawn itself must succeed")
            .expect("the child exited, so this is not a timeout");
        let elapsed = started.elapsed();

        assert!(
            output.status.success(),
            "the child's own exit status stays authoritative"
        );
        assert!(
            String::from_utf8_lossy(&output.stdout).contains("1.2.3"),
            "the bytes written before the deadline are kept: {:?}",
            output.stdout
        );
        assert!(
            elapsed < timeout * 3,
            "the drain must be bounded by the timeout; took {elapsed:?}"
        );
    }
}
