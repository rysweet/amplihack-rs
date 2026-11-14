use amplihack_utils::clock::{Clock, SystemClock};
/// Integration tests: AC9 no-Python probe — extended coverage (Issue #77).
///
/// These tests validate that the amplihack binary handles code-graph subcommands
/// (`index-code`, `index-scip`, `query-code`) correctly in a Python-free
/// environment.
///
/// # Status
///
/// All tests TC-01 through TC-08 pass. The probe script
/// (`scripts/probe-no-python.sh`) now covers TC-04 through TC-08, including
/// the populated native code-graph path. AC9 is satisfied for the currently
/// modeled no-Python CLI scenarios.
///
/// TC-06 (`query-code stats` on a fresh Kuzu DB) verifies no-crash and
/// no-Python-invocation; a non-zero exit code from Kuzu on an empty DB
/// is accepted as long as the process is not killed by a signal.
use std::fs;
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;

use crate::wait::{CEILING, wait_child};

/// Returns the path to the compiled `amplihack` binary.
fn amplihack_bin() -> PathBuf {
    // Hard `env!` (not `option_env!` with a fallback): Cargo builds the binary
    // as a prerequisite of this [[test]] target and exposes its exact path,
    // eliminating the "binary not found" race and ignoring stale target/ paths.
    PathBuf::from(env!("CARGO_BIN_EXE_amplihack"))
}

fn workspace_root() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop(); // tests/
    path.pop(); // workspace root
    path
}

fn clean_path_without_python() -> String {
    let original_path = std::env::var("PATH").unwrap_or_default();
    let clean_path: Vec<&str> = original_path
        .split(':')
        .filter(|dir| {
            !std::path::Path::new(dir).join("python").exists()
                && !std::path::Path::new(dir).join("python3").exists()
        })
        .collect();
    clean_path.join(":")
}

/// Build a `Command` that has Python stripped from its PATH, simulating the
/// probe-no-python.sh environment.  This is the environment in which every
/// AC9 smoke test must succeed.
fn cmd_without_python(bin: &PathBuf) -> Command {
    let mut cmd = crate::isolated_amplihack::isolated(bin);
    cmd.env("PATH", clean_path_without_python());
    cmd
}

fn cmd_with_failing_python_shims(bin: &PathBuf, shim_dir: &Path, python_log: &Path) -> Command {
    fs::create_dir_all(shim_dir).unwrap_or_else(|error| {
        panic!("failed to create shim dir {shim_dir:?}: {error}");
    });
    write_executable(
        &shim_dir.join("python"),
        &format!(
            "#!/bin/sh\necho python >> {}\nexit 97\n",
            python_log.display()
        ),
    );
    write_executable(
        &shim_dir.join("python3"),
        &format!(
            "#!/bin/sh\necho python3 >> {}\nexit 97\n",
            python_log.display()
        ),
    );

    let clean_path = clean_path_without_python();
    let path = if clean_path.is_empty() {
        shim_dir.display().to_string()
    } else {
        format!("{}:{clean_path}", shim_dir.display())
    };

    let mut cmd = crate::isolated_amplihack::isolated(bin);
    cmd.env("PATH", path);
    cmd
}

fn write_executable(path: &Path, content: &str) {
    fs::write(path, content).unwrap_or_else(|error| {
        panic!("failed to write executable {path:?}: {error}");
    });
    let mut permissions = fs::metadata(path)
        .unwrap_or_else(|error| panic!("failed to stat executable {path:?}: {error}"))
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).unwrap_or_else(|error| {
        panic!("failed to chmod executable {path:?}: {error}");
    });
}

fn strip_ansi(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0usize;

    while index < bytes.len() {
        if bytes[index] == 0x1b && index + 1 < bytes.len() && bytes[index + 1] == b'[' {
            index += 2;
            while index < bytes.len() {
                let byte = bytes[index];
                index += 1;
                if byte.is_ascii_alphabetic() {
                    break;
                }
            }
            continue;
        }

        if bytes[index] != b'\r' {
            output.push(bytes[index]);
        }
        index += 1;
    }

    String::from_utf8_lossy(&output).into_owned()
}

/// The text a terminal would show for `bytes`, without escape sequences.
fn screen(bytes: &[u8]) -> String {
    strip_ansi(&String::from_utf8_lossy(bytes))
}

/// The frames drawn in `text` that have ended, because a later frame has
/// begun. Every fleet TUI frame starts with its top-left corner `╔`; the
/// fragment before the first corner belongs to an earlier frame and is left
/// out, as is the last frame, which may still be arriving.
fn complete_frames(text: &str) -> Vec<&str> {
    let mut frames: Vec<&str> = text.split('╔').skip(1).collect();
    frames.pop();
    frames
}

/// One message from the pty reader thread.
enum PtyEvent {
    /// Bytes the child wrote to the terminal.
    Data(Vec<u8>),
    /// The stream ended: EOF, or EIO once every holder of the pty's child
    /// side has closed it.
    End,
}

/// The pty stream ended before the awaited screen text appeared.
#[derive(Debug)]
struct Closed {
    /// Everything the child wrote, without escape sequences.
    output: String,
}

/// Owns the pty master. A dedicated thread reads it, blocking, and passes
/// each chunk over a channel; [`PtyReader::wait_for`] blocks on that channel.
///
/// Success depends on screen events; the shared hang guard bounds a missing
/// event even when the child stays alive. Unwinding triggers [`KillOnDrop`].
struct PtyReader {
    events: mpsc::Receiver<PtyEvent>,
    output: Vec<u8>,
    ended: bool,
}

impl PtyReader {
    fn spawn(mut master: fs::File) -> Self {
        let (tx, events) = mpsc::channel();
        std::thread::spawn(move || {
            let mut chunk = [0u8; 4096];
            loop {
                match master.read(&mut chunk) {
                    Ok(0) => break,
                    Ok(read) => {
                        if tx.send(PtyEvent::Data(chunk[..read].to_vec())).is_err() {
                            return;
                        }
                    }
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                    // EIO is how Linux reports that the child side is closed;
                    // any other error also ends the stream.
                    Err(_) => break,
                }
            }
            let _ = tx.send(PtyEvent::End);
        });
        Self {
            events,
            output: Vec::new(),
            ended: false,
        }
    }

    /// Blocks until the output so far satisfies `predicate` and returns it,
    /// or returns [`Closed`] with the captured output once the stream ends.
    fn wait_for(&mut self, mut predicate: impl FnMut(&[u8]) -> bool) -> Result<&[u8], Closed> {
        self.wait_for_with(CEILING, &SystemClock, &mut predicate)
    }

    fn wait_for_with(
        &mut self,
        ceiling: std::time::Duration,
        clock: &dyn Clock,
        mut predicate: impl FnMut(&[u8]) -> bool,
    ) -> Result<&[u8], Closed> {
        let deadline = clock.now() + ceiling;
        loop {
            if predicate(&self.output) {
                return Ok(&self.output);
            }
            if self.ended {
                return Err(Closed {
                    output: screen(&self.output),
                });
            }
            let now = clock.now();
            assert!(
                now < deadline,
                "hung: waiting for PTY output or EOF for {ceiling:?}; output tail:\n{}",
                crate::wait::tail_of(&self.output)
            );
            match self
                .events
                .recv_timeout(deadline.saturating_duration_since(now))
            {
                Ok(PtyEvent::Data(bytes)) => self.output.extend_from_slice(&bytes),
                Ok(PtyEvent::End) | Err(mpsc::RecvTimeoutError::Disconnected) => self.ended = true,
                Err(mpsc::RecvTimeoutError::Timeout) => panic!(
                    "hung: waiting for PTY output or EOF for {ceiling:?}; output tail:\n{}",
                    crate::wait::tail_of(&self.output)
                ),
            }
        }
    }

    /// Reads to the end of the stream.
    fn read_to_end(&mut self) {
        let _ = self.wait_for(|_| false);
    }
}

#[test]
fn pty_wait_returns_on_matching_output_and_reports_eof() {
    let (sender, events) = mpsc::channel();
    sender.send(PtyEvent::Data(b"ready".to_vec())).unwrap();
    sender.send(PtyEvent::End).unwrap();
    let mut reader = PtyReader {
        events,
        output: Vec::new(),
        ended: false,
    };
    assert_eq!(
        reader.wait_for(|bytes| bytes == b"ready").unwrap(),
        b"ready"
    );
    assert_eq!(reader.wait_for(|_| false).unwrap_err().output, "ready");
}

#[test]
#[should_panic(expected = "hung: waiting for PTY output or EOF")]
fn pty_wait_bounds_a_live_silent_stream() {
    let (_sender, events) = mpsc::channel();
    let mut reader = PtyReader {
        events,
        output: Vec::new(),
        ended: false,
    };
    let _ = reader.wait_for_with(std::time::Duration::ZERO, &SystemClock, |_| false);
}

#[test]
#[should_panic(expected = "hung: waiting for PTY output or EOF")]
fn pty_redraws_do_not_reset_the_wait_ceiling() {
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    struct TickingClock(Mutex<Instant>);
    impl Clock for TickingClock {
        fn now(&self) -> Instant {
            let mut now = self.0.lock().unwrap();
            let result = *now;
            *now += Duration::from_secs(1);
            result
        }
        fn sleep(&self, _: Duration) {
            panic!("PTY waits receive events instead of sleeping");
        }
    }
    let (sender, events) = mpsc::channel();
    for _ in 0..10 {
        sender.send(PtyEvent::Data(b"redraw".to_vec())).unwrap();
    }
    // Keep the sender alive: the wait must reach its original deadline while
    // redraws are still queued, rather than ending through a disconnected EOF.
    let mut reader = PtyReader {
        events,
        output: Vec::new(),
        ended: false,
    };
    let clock = TickingClock(Mutex::new(Instant::now()));
    let _ = reader.wait_for_with(Duration::from_secs(5), &clock, |_| false);
}

/// Kills and reaps the child if it is still running when the test drops it,
/// so a failed test leaves nothing behind and the reader thread sees the end
/// of the stream.
struct KillOnDrop(Child);

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        if let Ok(None) = self.0.try_wait() {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

/// Skip the test if the binary has not been built yet.
macro_rules! require_binary {
    ($bin:expr) => {
        if !$bin.exists() {
            eprintln!(
                "SKIP: amplihack binary not found at {:?} — run `cargo build` first.",
                $bin
            );
            return;
        }
    };
}

// ── Pre-existing smoke tests (TC-01 / TC-02 / TC-03) ─────────────────────
//
// These are the tests already implemented in cli_launch_test.rs and the
// current probe script.  They are mirrored here to establish the baseline and
// to confirm they continue to pass in a Python-free PATH.

/// TC-01 (pre-existing): `amplihack --version` must exit 0 without Python.
#[test]
fn tc01_version_exits_zero_without_python() {
    let bin = amplihack_bin();
    require_binary!(bin);
    let status = cmd_without_python(&bin)
        .arg("--version")
        .status()
        .expect("failed to spawn");
    assert!(
        status.success(),
        "TC-01 FAIL: --version exited {:?} in Python-free environment",
        status.code()
    );
}

/// TC-02 (pre-existing): `amplihack --help` must exit 0 without Python.
#[test]
fn tc02_help_exits_zero_without_python() {
    let bin = amplihack_bin();
    require_binary!(bin);
    let status = cmd_without_python(&bin)
        .arg("--help")
        .status()
        .expect("failed to spawn");
    assert!(
        status.success(),
        "TC-02 FAIL: --help exited {:?} in Python-free environment",
        status.code()
    );
}

/// TC-03 (pre-existing): `amplihack fleet --help` must exit 0 without Python.
#[test]
fn tc03_fleet_help_exits_zero_without_python() {
    let bin = amplihack_bin();
    require_binary!(bin);
    let status = cmd_without_python(&bin)
        .args(["fleet", "--help"])
        .status()
        .expect("failed to spawn");
    assert!(
        status.success(),
        "TC-03 FAIL: fleet --help exited {:?} in Python-free environment",
        status.code()
    );
}

// ── New tests (TC-04 through TC-07) ──────────────────────────────────────
//
// These tests cover the Issue #77 AC9 extension.  All four pass.

/// TC-04: `amplihack index-code --help` must exit 0 without Python.
///
/// Verifies `index-code` is registered in the CLI router and the help page
/// renders without invoking a Python interpreter.
#[test]
fn tc04_index_code_help_exits_zero_without_python() {
    let bin = amplihack_bin();
    require_binary!(bin);

    let output = cmd_without_python(&bin)
        .args(["index-code", "--help"])
        .output()
        .expect("failed to spawn");

    assert!(
        output.status.success(),
        "TC-04 FAIL: `index-code --help` exited {:?} in Python-free environment.\n\
         stdout: {}\nstderr: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

/// TC-05: `amplihack query-code --help` must exit 0 without Python.
///
/// Verifies `query-code` is registered in the CLI router and the help page
/// renders without invoking a Python interpreter.
#[test]
fn tc05_query_code_help_exits_zero_without_python() {
    let bin = amplihack_bin();
    require_binary!(bin);

    let output = cmd_without_python(&bin)
        .args(["query-code", "--help"])
        .output()
        .expect("failed to spawn");

    assert!(
        output.status.success(),
        "TC-05 FAIL: `query-code --help` exited {:?} in Python-free environment.\n\
         stdout: {}\nstderr: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

/// TC-06: `amplihack query-code stats` against a fresh Kuzu DB must not crash
/// and must not invoke a Python interpreter.
///
/// A non-zero exit code from Kuzu on an empty database is acceptable.
/// The test asserts: (1) process terminates via exit code, not a signal;
/// (2) output contains no evidence of Python invocation.
#[test]
fn tc06_query_code_stats_smoke_on_fresh_db_without_python() {
    let bin = amplihack_bin();
    require_binary!(bin);

    // Create a temp file path for the code-graph DB. We pass the path to the
    // binary so it creates a fresh database — we do NOT pre-populate it.
    let temp_dir = tempfile::TempDir::new().expect("failed to create tempdir");
    let db_path = temp_dir.path().join("probe_tc06.lbug");

    let output = cmd_without_python(&bin)
        .args([
            "query-code",
            "--db-path",
            db_path.to_str().expect("non-UTF-8 temp path"),
            "stats",
        ])
        .output()
        .expect("failed to spawn");

    // We do NOT assert success — Kuzu may return an error for an empty DB.
    // We DO assert the process did not terminate via a signal (crash), which
    // would be represented by `status.code()` returning None on Unix.
    assert!(
        output.status.code().is_some(),
        "TC-06 FAIL: `query-code stats` was killed by a signal (crash) in \
         Python-free environment.\nstderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // The output must not contain evidence of Python invocation.
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !combined.contains("python: command not found")
            && !combined.contains("python3: command not found")
            && !combined.contains("No such file or directory: 'python")
            && !combined.contains("ModuleNotFoundError"),
        "TC-06 FAIL: output contains evidence of Python invocation:\n{combined}"
    );
}

/// TC-07: `amplihack index-scip --help` must exit 0 without Python.
///
/// index-scip is the SCIP-based indexing command (invokes external Go binaries
/// like scip-python, NOT a Python interpreter).  This test confirms the help
/// page is available in a Python-free environment.
#[test]
fn tc07_index_scip_help_exits_zero_without_python() {
    let bin = amplihack_bin();
    require_binary!(bin);

    let output = cmd_without_python(&bin)
        .args(["index-scip", "--help"])
        .output()
        .expect("failed to spawn");

    assert!(
        output.status.success(),
        "TC-07 FAIL: `index-scip --help` exited {:?} in Python-free environment.\n\
         stdout: {}\nstderr: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

/// TC-08: native code-graph import and query must work end-to-end with Python
/// forcibly broken on PATH.
#[test]
fn tc08_index_code_and_query_code_work_without_python() {
    let bin = amplihack_bin();
    require_binary!(bin);

    let temp_dir = tempfile::TempDir::new().expect("failed to create tempdir");
    let shim_dir = temp_dir.path().join("bin");
    let python_log = temp_dir.path().join("python.log");
    let db_path = temp_dir.path().join("probe_tc08.lbug");
    let json_path = temp_dir.path().join("blarify.json");

    fs::write(
        &json_path,
        serde_json::json!({
            "files": [
                {"path":"src/example/module.py","language":"python","lines_of_code":10},
                {"path":"src/example/utils.py","language":"python","lines_of_code":5}
            ],
            "classes": [
                {"id":"class:Example","name":"Example","file_path":"src/example/module.py","line_number":1}
            ],
            "functions": [
                {"id":"func:Example.process","name":"process","file_path":"src/example/module.py","line_number":2,"class_id":"class:Example"},
                {"id":"func:helper","name":"helper","file_path":"src/example/utils.py","line_number":1}
            ],
            "imports": [],
            "relationships": [
                {"type":"CALLS","source_id":"func:Example.process","target_id":"func:helper"}
            ]
        })
        .to_string(),
    )
    .expect("failed to write blarify fixture");

    let index_output = cmd_with_failing_python_shims(&bin, &shim_dir, &python_log)
        .args([
            "index-code",
            json_path.to_str().expect("non-UTF-8 fixture path"),
            "--db-path",
            db_path.to_str().expect("non-UTF-8 db path"),
        ])
        .output()
        .expect("failed to run index-code");
    assert!(
        index_output.status.success(),
        "TC-08 FAIL: `index-code` exited {:?}.\nstdout: {}\nstderr: {}",
        index_output.status.code(),
        String::from_utf8_lossy(&index_output.stdout),
        String::from_utf8_lossy(&index_output.stderr)
    );

    let stats_output = cmd_with_failing_python_shims(&bin, &shim_dir, &python_log)
        .args([
            "query-code",
            "--db-path",
            db_path.to_str().expect("non-UTF-8 db path"),
            "--json",
            "stats",
        ])
        .output()
        .expect("failed to run query-code stats");
    assert!(
        stats_output.status.success(),
        "TC-08 FAIL: `query-code stats` exited {:?}.\nstdout: {}\nstderr: {}",
        stats_output.status.code(),
        String::from_utf8_lossy(&stats_output.stdout),
        String::from_utf8_lossy(&stats_output.stderr)
    );
    let stats_json: serde_json::Value =
        serde_json::from_slice(&stats_output.stdout).expect("stats output must be valid JSON");
    assert_eq!(stats_json["files"], 2);
    assert_eq!(stats_json["classes"], 1);
    assert_eq!(stats_json["functions"], 2);

    let search_output = cmd_with_failing_python_shims(&bin, &shim_dir, &python_log)
        .args([
            "query-code",
            "--db-path",
            db_path.to_str().expect("non-UTF-8 db path"),
            "--json",
            "search",
            "helper",
        ])
        .output()
        .expect("failed to run query-code search");
    assert!(
        search_output.status.success(),
        "TC-08 FAIL: `query-code search helper` exited {:?}.\nstdout: {}\nstderr: {}",
        search_output.status.code(),
        String::from_utf8_lossy(&search_output.stdout),
        String::from_utf8_lossy(&search_output.stderr)
    );
    let search_json: serde_json::Value =
        serde_json::from_slice(&search_output.stdout).expect("search output must be valid JSON");
    assert!(
        search_json
            .as_array()
            .expect("search output must be an array")
            .iter()
            .any(|entry| entry["type"] == "function" && entry["name"] == "helper"),
        "TC-08 FAIL: query-code search did not return helper.\n{}",
        String::from_utf8_lossy(&search_output.stdout)
    );

    let callers_output = cmd_with_failing_python_shims(&bin, &shim_dir, &python_log)
        .args([
            "query-code",
            "--db-path",
            db_path.to_str().expect("non-UTF-8 db path"),
            "--json",
            "callers",
            "helper",
        ])
        .output()
        .expect("failed to run query-code callers");
    assert!(
        callers_output.status.success(),
        "TC-08 FAIL: `query-code callers helper` exited {:?}.\nstdout: {}\nstderr: {}",
        callers_output.status.code(),
        String::from_utf8_lossy(&callers_output.stdout),
        String::from_utf8_lossy(&callers_output.stderr)
    );
    let callers_json: serde_json::Value =
        serde_json::from_slice(&callers_output.stdout).expect("callers output must be valid JSON");
    assert!(
        callers_json
            .as_array()
            .expect("callers output must be an array")
            .iter()
            .any(|entry| entry["caller"] == "process" && entry["callee"] == "helper"),
        "TC-08 FAIL: query-code callers did not return process -> helper.\n{}",
        String::from_utf8_lossy(&callers_output.stdout)
    );

    let python_hits = fs::read_to_string(&python_log).unwrap_or_default();
    assert!(
        python_hits.trim().is_empty(),
        "TC-08 FAIL: native code-graph path touched python unexpectedly:\n{python_hits}"
    );
}

// ── Probe script execution test ───────────────────────────────────────────

// ── Probe script CONTENT checks (AC9 extension validation) ───────────────
//
// These tests verify that the probe shell script contains the AC9 test
// cases (TC-04 through TC-07).  All pass — the script has been extended.

fn read_probe_script() -> String {
    let mut p = workspace_root();
    p.push("scripts/probe-no-python.sh");
    std::fs::read_to_string(&p)
        .unwrap_or_else(|_| panic!("probe script not found at {p:?} — run from workspace root"))
}

fn probe_script_path() -> PathBuf {
    let mut path = workspace_root();
    path.push("scripts/probe-no-python.sh");
    path
}

fn copy_probe_script_to_temp() -> (tempfile::TempDir, PathBuf) {
    let temp_dir = tempfile::TempDir::new().expect("failed to create tempdir");
    let script_dir = temp_dir.path().join("scripts");
    fs::create_dir_all(&script_dir).expect("failed to create temporary scripts dir");
    let script_path = script_dir.join("probe-no-python.sh");
    fs::copy(probe_script_path(), &script_path).expect("failed to copy probe script");
    (temp_dir, script_path)
}

fn fake_amplihack_script(command_log: &Path) -> String {
    format!(
        r#"#!/bin/sh
printf '%s\n' "$*" >> {command_log}

case "$*" in
  "--version"|"--help"|"fleet --help"|"doctor --help"|"recipe --help"|"index-code --help"|"query-code --help"|"index-scip --help")
    exit 0
    ;;
esac

if [ "$1" = "index-code" ]; then
  exit 0
fi

if [ "$1" = "query-code" ]; then
  case "$*" in
    *" callers helper"*)
      printf '%s\n' '[{{"caller":"process","callee":"helper"}}]'
      exit 0
      ;;
    *" search helper"*)
      printf '%s\n' '[{{"type":"function","name":"helper"}}]'
      exit 0
      ;;
    *" stats"*)
      printf '%s\n' '{{"files":2,"classes":1,"functions":2}}'
      exit 0
      ;;
  esac
fi

echo "unexpected fake amplihack invocation: $*" >&2
exit 64
"#,
        command_log = shell_quote_path(command_log)
    )
}

fn shell_quote_path(path: &Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', "'\\''"))
}

fn prepend_path(dir: &Path) -> String {
    let original = std::env::var("PATH").unwrap_or_default();
    if original.is_empty() {
        dir.display().to_string()
    } else {
        format!("{}:{original}", dir.display())
    }
}

/// TC-04 content gate: the probe script must call `index-code --help`.
#[test]
fn probe_script_content_contains_tc04_index_code_help() {
    let script = read_probe_script();
    assert!(
        script.contains("index-code") && script.contains("--help"),
        "FAIL TC-04 content gate: probe-no-python.sh must include an \
         'index-code --help' smoke test.\n\
         Add: run_smoke \"TC-04 index-code --help\"  \
              \"${{BINARY}}\" index-code --help"
    );
}

/// TC-05 content gate: the probe script must call `query-code --help`.
#[test]
fn probe_script_content_contains_tc05_query_code_help() {
    let script = read_probe_script();
    assert!(
        script.contains("query-code") && script.contains("--help"),
        "FAIL TC-05 content gate: probe-no-python.sh must include a \
         'query-code --help' smoke test.\n\
         Add: run_smoke \"TC-05 query-code --help\"  \
              \"${{BINARY}}\" query-code --help"
    );
}

/// TC-06 content gate: the probe script must contain a mktemp-based
/// `query-code stats` smoke test with a `trap ... EXIT` cleanup.
#[test]
fn probe_script_content_contains_tc06_memory_smoke_with_mktemp() {
    let script = read_probe_script();
    let has_mktemp = script.contains("mktemp");
    let has_query_code = script.contains("query-code");
    let has_trap = script.contains("trap");
    assert!(
        has_mktemp && has_query_code && has_trap,
        "FAIL TC-06 content gate: probe-no-python.sh must include a \
         mktemp-based 'query-code stats' smoke test with a trap EXIT cleanup.\n\
         Missing: mktemp={has_mktemp}, query-code={has_query_code}, trap={has_trap}"
    );
}

/// TC-07 content gate: the probe script must call `index-scip --help`.
#[test]
fn probe_script_content_contains_tc07_index_scip_help() {
    let script = read_probe_script();
    assert!(
        script.contains("index-scip"),
        "FAIL TC-07 content gate: probe-no-python.sh must include an \
         'index-scip --help' smoke test.\n\
         Add: run_smoke \"TC-07 index-scip --help\"  \
              \"${{BINARY}}\" index-scip --help"
    );
}

/// TC-08 content gate: the probe script must exercise populated native
/// `index-code` + `query-code` flows using the backend-neutral `--db-path`
/// flag.
#[test]
fn probe_script_content_contains_tc08_populated_code_graph_probe() {
    let script = read_probe_script();
    let has_tc08 = script.contains("TC-08");
    let has_index = script.contains("index-code");
    let has_db_path = script.contains("--db-path");
    let has_search = script.contains("query-code --db-path") && script.contains("search helper");
    let has_callers = script.contains("callers helper");
    assert!(
        has_tc08 && has_index && has_db_path && has_search && has_callers,
        "FAIL TC-08 content gate: probe-no-python.sh must include a populated \
         native code-graph smoke test using --db-path plus search/callers checks.\n\
         Missing: tc08={has_tc08}, index={has_index}, db_path={has_db_path}, \
         search={has_search}, callers={has_callers}"
    );
}

/// Reuse contract: a valid AMPLIHACK_PROBE_BIN must be resolved before PATH
/// stripping and must skip the internal cargo build, even when --release is
/// supplied.
#[test]
fn probe_script_uses_valid_amplihack_probe_bin_without_internal_build_even_release() {
    let (_temp_dir, script) = copy_probe_script_to_temp();
    let test_root = script
        .parent()
        .and_then(Path::parent)
        .expect("temporary script must be under scripts/");
    let bin_dir = test_root.join("bin");
    fs::create_dir_all(&bin_dir).expect("failed to create fake bin dir");

    let command_log = test_root.join("amplihack-commands.log");
    let cargo_log = test_root.join("cargo.log");
    let fake_amplihack = test_root.join("reusable-amplihack");
    write_executable(&fake_amplihack, &fake_amplihack_script(&command_log));
    write_executable(
        &bin_dir.join("cargo"),
        &format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> {}\nexit 97\n",
            shell_quote_path(&cargo_log)
        ),
    );

    let output = Command::new("bash")
        .arg(&script)
        .arg("--release")
        .env("AMPLIHACK_PROBE_BIN", &fake_amplihack)
        .env("PATH", prepend_path(&bin_dir))
        .output()
        .expect("failed to run probe script");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!("{stdout}{stderr}");
    assert!(
        output.status.success(),
        "probe-no-python.sh should use valid AMPLIHACK_PROBE_BIN and skip cargo, \
         but exited {:?}.\nOutput:\n{combined}",
        output.status.code()
    );
    assert!(
        !combined.contains("==> Building amplihack-rs"),
        "valid AMPLIHACK_PROBE_BIN must skip the internal cargo build.\nOutput:\n{combined}"
    );
    assert!(
        !cargo_log.exists(),
        "valid AMPLIHACK_PROBE_BIN must not invoke cargo; cargo log:\n{}",
        fs::read_to_string(&cargo_log).unwrap_or_default()
    );

    let commands = fs::read_to_string(&command_log).expect("fake binary should have been used");
    assert!(
        commands.contains("--version")
            && commands.contains("query-code --db-path")
            && commands.contains("index-code")
            && commands.contains("callers helper"),
        "probe did not exercise the expected smoke-test commands through \
         AMPLIHACK_PROBE_BIN:\n{commands}"
    );
}

/// Fallback contract: an invalid AMPLIHACK_PROBE_BIN must produce a visible
/// warning and then preserve standalone cargo build behavior.
#[test]
fn probe_script_warns_and_falls_back_when_amplihack_probe_bin_is_invalid() {
    let (_temp_dir, script) = copy_probe_script_to_temp();
    let test_root = script
        .parent()
        .and_then(Path::parent)
        .expect("temporary script must be under scripts/");
    let bin_dir = test_root.join("bin");
    fs::create_dir_all(&bin_dir).expect("failed to create fake bin dir");

    let command_log = test_root.join("amplihack-commands.log");
    let cargo_log = test_root.join("cargo.log");
    let fake_amplihack = test_root.join("built-amplihack");
    write_executable(&fake_amplihack, &fake_amplihack_script(&command_log));
    write_executable(
        &bin_dir.join("cargo"),
        &format!(
            r#"#!/bin/sh
printf '%s\n' "$*" >> {cargo_log}
repo_root=''
profile='debug'
while [ "$#" -gt 0 ]; do
  case "$1" in
    --manifest-path)
      shift
      repo_root="$(dirname "$1")"
      ;;
    --release)
      profile='release'
      ;;
  esac
  shift
done
if [ -z "$repo_root" ]; then
  echo "missing --manifest-path" >&2
  exit 65
fi
mkdir -p "$repo_root/target/$profile"
cp {fake_amplihack} "$repo_root/target/$profile/amplihack"
chmod 755 "$repo_root/target/$profile/amplihack"
exit 0
"#,
            cargo_log = shell_quote_path(&cargo_log),
            fake_amplihack = shell_quote_path(&fake_amplihack)
        ),
    );

    let output = Command::new("bash")
        .arg(&script)
        .env("AMPLIHACK_PROBE_BIN", test_root.join("missing-amplihack"))
        .env("PATH", prepend_path(&bin_dir))
        .output()
        .expect("failed to run probe script");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!("{stdout}{stderr}");
    assert!(
        output.status.success(),
        "probe-no-python.sh should warn and fall back to cargo when \
         AMPLIHACK_PROBE_BIN is invalid, but exited {:?}.\nOutput:\n{combined}",
        output.status.code()
    );
    assert!(
        combined.contains("WARNING") && combined.contains("AMPLIHACK_PROBE_BIN"),
        "invalid AMPLIHACK_PROBE_BIN must emit a clear warning before fallback.\n\
         Output:\n{combined}"
    );
    assert!(
        fs::read_to_string(&cargo_log)
            .unwrap_or_default()
            .contains("build"),
        "invalid AMPLIHACK_PROBE_BIN must preserve standalone cargo build fallback"
    );
}

/// Validate that the probe shell script exits 0 and covers TC-04 through TC-08.
///
/// Checks both:
///   1. The script exits 0 (all smoke tests pass)
///   2. The output proves TC-04 through TC-08 were exercised
///
/// Skipped when the script is not executable (e.g., fresh clone without
/// execute bit set).
#[test]
fn probe_script_exits_zero_and_covers_tc04_through_tc08() {
    let script = probe_script_path();

    if !script.exists() {
        eprintln!("SKIP: probe script not found at {script:?}");
        return;
    }
    if !{
        use std::os::unix::fs::PermissionsExt;
        script
            .metadata()
            .map(|m| m.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    } {
        eprintln!("SKIP: probe script is not executable at {script:?}");
        return;
    }

    let bin = amplihack_bin();
    require_binary!(bin);

    let output = Command::new("bash")
        .arg(&script)
        .env("AMPLIHACK_PROBE_BIN", &bin)
        .output()
        .expect("failed to run probe script");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!("{stdout}{stderr}");

    // Gate 1: script exits 0 (all smoke tests pass).
    assert!(
        output.status.success(),
        "probe-no-python.sh exited {:?} — one or more smoke tests failed.\n\
         Output:\n{combined}",
        output.status.code()
    );
    assert!(
        !combined.contains("==> Building amplihack-rs"),
        "probe-no-python.sh must reuse AMPLIHACK_PROBE_BIN during integration \
         tests instead of triggering a nested cargo build.\nOutput:\n{combined}"
    );

    // Gate 2: script output proves it ran TC-04 through TC-08.
    let has_tc04 = combined.contains("index-code");
    let has_tc05 = combined.contains("query-code");
    let has_tc06 = combined.contains("mktemp") || combined.contains("stats");
    let has_tc07 = combined.contains("index-scip");
    let has_tc08 = combined.contains("TC-08") && combined.contains("populated graph");
    assert!(
        has_tc04 && has_tc05 && has_tc06 && has_tc07 && has_tc08,
        "FAIL: probe script ran successfully but did not exercise TC-04 through TC-08.\n\
         Missing: tc04(index-code)={has_tc04}, tc05(query-code)={has_tc05}, \
         tc06(stats/mktemp)={has_tc06}, tc07(index-scip)={has_tc07}, \
         tc08(populated graph)={has_tc08}\n\
         Full output:\n{combined}"
    );
}

struct FleetTuiProbe {
    _temp_dir: tempfile::TempDir,
    /// A second handle on the pty master, for keystrokes.
    input: fs::File,
    reader: PtyReader,
    child: KillOnDrop,
    python_log: PathBuf,
    send_log: PathBuf,
    create_log: PathBuf,
    queue_path: PathBuf,
    dashboard_path: PathBuf,
}

impl FleetTuiProbe {
    fn new(bin: &Path, reasoner_json: Option<&str>) -> Self {
        Self::new_with_fixture(bin, reasoner_json, None, None, None)
    }

    fn new_with_existing_vms(
        bin: &Path,
        reasoner_json: Option<&str>,
        existing_vms: Option<&str>,
    ) -> Self {
        Self::new_with_fixture(bin, reasoner_json, existing_vms, None, None)
    }

    fn new_with_reasoner_script(bin: &Path, reasoner_script: &str) -> Self {
        Self::new_with_fixture(bin, None, None, Some(reasoner_script), None)
    }

    fn new_with_custom_azlin_script(
        bin: &Path,
        reasoner_json: Option<&str>,
        azlin_script: &str,
    ) -> Self {
        Self::new_with_fixture(bin, reasoner_json, None, None, Some(azlin_script))
    }

    fn new_with_fixture(
        bin: &Path,
        reasoner_json: Option<&str>,
        existing_vms: Option<&str>,
        reasoner_script: Option<&str>,
        azlin_script_override: Option<&str>,
    ) -> Self {
        let temp_dir = tempfile::TempDir::new().expect("failed to create tempdir");
        let fake_bin = temp_dir.path().join("bin");
        fs::create_dir_all(&fake_bin).expect("failed to create fake bin dir");

        let python_log = temp_dir.path().join("python.log");
        let send_log = temp_dir.path().join("send.log");
        let create_log = temp_dir.path().join("create.log");
        let queue_path = temp_dir.path().join(".amplihack/fleet/task_queue.json");
        let dashboard_path = temp_dir.path().join(".amplihack/fleet/dashboard.json");
        let azlin = temp_dir.path().join("azlin");
        let list_json = if existing_vms.is_some() {
            r#"[{"name":"vm-1","status":"Running","region":"westus2","session_name":"vm-1"},{"name":"vm-2","status":"Running","region":"eastus","session_name":"vm-2"}]"#
        } else {
            r#"[{"name":"vm-1","status":"Running","region":"westus2","session_name":"vm-1"}]"#
        };
        let vm2_block = if existing_vms.is_some() {
            r#"
if [ "$1" = "connect" ] && [ "$2" = "vm-2" ]; then
  case "$*" in
    *"PANE_START"*)
      printf '%s\n' \
        '===SESSION:copilot-9===' \
        'CWD:/tmp/excluded' \
        'CMD:copilot' \
        'REPO:https://github.com/org/excluded.git' \
        'BRANCH:side-quest' \
        'LAST_MSG:Waiting for operator review' \
        '===DONE==='
      exit 0
      ;;
    *"tmux list-sessions"*)
      printf '%s\n' 'copilot-9|||1|||0'
      exit 0
      ;;
    *"===TMUX==="*)
      cat <<'EOF'
===TMUX===
Working outside fleet
===CWD===
/tmp/excluded
===GIT===
BRANCH:side-quest
REMOTE:
MODIFIED:
===TRANSCRIPT===
---EARLY---
Investigating unrelated task
---RECENT---
Waiting for operator review
===HEALTH===
mem=12% disk=18% load=0.12
===OBJECTIVES===
===END===
EOF
      exit 0
      ;;
    *"tmux capture-pane"*)
      printf '%s\n' 'Working outside fleet'
      exit 0
      ;;
  esac
fi
 "#
        } else {
            ""
        };

        write_executable(
            &fake_bin.join("python"),
            &format!(
                "#!/bin/sh\necho python >> {}\nexit 97\n",
                python_log.display()
            ),
        );
        write_executable(
            &fake_bin.join("python3"),
            &format!(
                "#!/bin/sh\necho python3 >> {}\nexit 97\n",
                python_log.display()
            ),
        );
        let default_azlin_script = format!(
            r#"#!/bin/sh
if [ "$1" = "list" ] && [ "$2" = "--json" ]; then
  printf '%s\n' '{}'
  exit 0
fi
    if [ "$1" = "connect" ] && [ "$2" = "vm-1" ]; then
      case "$*" in
        *"PANE_START"*)
          printf '%s\n' \
            '===SESSION:claude-1===' \
            'CWD:/tmp/demo' \
            'CMD:claude' \
            'REPO:https://github.com/org/demo.git' \
            'BRANCH:main' \
            'LAST_MSG:Awaiting operator confirmation' \
            '===DONE==='
          exit 0
          ;;
        *"tmux list-sessions"*)
          printf '%s\n' 'claude-1|||1|||0'
          exit 0
          ;;
    *"===TMUX==="*)
      cat <<'EOF'
===TMUX===
Proceed with deploy? [y/n]
===CWD===
/tmp/demo
===GIT===
BRANCH:main
REMOTE:
MODIFIED:
===TRANSCRIPT===
---EARLY---
User requested deployment help
---RECENT---
Awaiting operator confirmation
===HEALTH===
mem=10% disk=20% load=0.10
===OBJECTIVES===
===END===
EOF
      exit 0
      ;;
    *"tmux capture-pane"*)
      printf '%s\n' \
        'FULL DETAIL: deployment checklist open' \
        'Line 2: awaiting explicit operator input'
      exit 0
      ;;
    *"tmux send-keys"*)
      printf '%s\n' "$*" >> {}
      exit 0
      ;;
    *"tmux new-session -d -s"*)
      printf '%s\n' "$*" >> {}
      exit 0
      ;;
   esac
fi
{}
exit 1
"#,
            list_json,
            send_log.display(),
            create_log.display(),
            vm2_block
        );
        write_executable(
            &azlin,
            azlin_script_override.unwrap_or(&default_azlin_script),
        );

        let clean_path = clean_path_without_python();
        let path = if clean_path.is_empty() {
            fake_bin.display().to_string()
        } else {
            format!("{}:{clean_path}", fake_bin.display())
        };

        let mut master_fd = 0;
        let mut slave_fd = 0;
        let rc = unsafe {
            libc::openpty(
                &mut master_fd,
                &mut slave_fd,
                std::ptr::null_mut(),
                // `*mut` on macOS and `*const` on Linux; `*mut` coerces to both.
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        assert_eq!(rc, 0, "failed to allocate PTY");

        let master = unsafe { fs::File::from_raw_fd(master_fd) };
        let stdin_file = unsafe { fs::File::from_raw_fd(slave_fd) };
        let stdout_fd = unsafe { libc::dup(stdin_file.as_raw_fd()) };
        assert!(stdout_fd >= 0, "failed to dup PTY slave for stdout");
        let stderr_fd = unsafe { libc::dup(stdin_file.as_raw_fd()) };
        assert!(stderr_fd >= 0, "failed to dup PTY slave for stderr");
        let stdout_file = unsafe { fs::File::from_raw_fd(stdout_fd) };
        let stderr_file = unsafe { fs::File::from_raw_fd(stderr_fd) };

        let mut command = crate::isolated_amplihack::isolated(bin);
        command
            .args(["fleet", "tui", "--interval", "1", "--capture-lines", "10"])
            .env("PATH", path)
            .env("AZLIN_PATH", &azlin)
            .env("HOME", temp_dir.path())
            .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
            .stdin(Stdio::from(stdin_file))
            .stdout(Stdio::from(stdout_file))
            .stderr(Stdio::from(stderr_file));

        if let Some(existing_vms) = existing_vms {
            command.env("AMPLIHACK_FLEET_EXISTING_VMS", existing_vms);
        }

        let default_reasoner_json = r#"{"action":"wait","confidence":0.85,"reasoning":"Need operator confirmation","input_text":"y\n"}"#;
        let reasoner = fake_bin.join("claude");
        if let Some(script) = reasoner_script {
            write_executable(&reasoner, script);
        } else if let Some(json) = reasoner_json {
            write_executable(&reasoner, &format!("#!/bin/sh\nprintf '%s\\n' '{json}'\n"));
        } else {
            write_executable(
                &reasoner,
                &format!("#!/bin/sh\nprintf '%s\\n' '{default_reasoner_json}'\n"),
            );
        }
        command
            .env("AMPLIHACK_FLEET_REASONER_BINARY_PATH", &reasoner)
            .env("AMPLIHACK_CLAUDE_BINARY_PATH", reasoner);

        let child = KillOnDrop(command.spawn().expect("failed to spawn fleet tui"));
        // The child holds its own copies of the pty's child side; the parent's
        // went with `command`, so the stream ends when the child's do.
        drop(command);
        let input = master.try_clone().expect("failed to clone the PTY master");

        Self {
            _temp_dir: temp_dir,
            input,
            reader: PtyReader::spawn(master),
            child,
            python_log,
            send_log,
            create_log,
            queue_path,
            dashboard_path,
        }
    }

    /// Where the next output will start; waits that take it look only at
    /// what the TUI draws after this point.
    fn mark(&self) -> usize {
        self.reader.output.len()
    }

    /// Blocks until the screen text drawn since `mark` satisfies `cond`, and
    /// returns that text. Panics with everything captured if the TUI closes
    /// its terminal first.
    fn wait_since(
        &mut self,
        mark: usize,
        what: &str,
        mut cond: impl FnMut(&str) -> bool,
    ) -> String {
        match self
            .reader
            .wait_for(|out| cond(&screen(&out[mark.min(out.len())..])))
        {
            Ok(out) => screen(&out[mark..]),
            Err(closed) => panic!(
                "the fleet TUI closed its terminal before {what} appeared:\n{}",
                closed.output
            ),
        }
    }

    /// Blocks until `needle` is anywhere in the output so far.
    fn wait_for_text(&mut self, needle: &str) -> String {
        self.wait_since(0, needle, |text| text.contains(needle))
    }

    fn send(&mut self, bytes: &[u8]) {
        self.input
            .write_all(bytes)
            .unwrap_or_else(|error| panic!("failed to send PTY input {bytes:?}: {error}"));
    }

    /// Sends `bytes`, then blocks until the TUI draws `needle`.
    fn send_and_wait_for(&mut self, bytes: &[u8], needle: &str) -> String {
        self.send_and_wait_until(bytes, needle, |text| text.contains(needle))
    }

    /// Sends `bytes`, then blocks until the text drawn after them satisfies
    /// `cond`.
    fn send_and_wait_until(
        &mut self,
        bytes: &[u8],
        what: &str,
        cond: impl FnMut(&str) -> bool,
    ) -> String {
        let mark = self.mark();
        self.send(bytes);
        self.wait_since(mark, what, cond)
    }

    /// Sends `bytes`, then blocks until a whole frame drawn after them
    /// satisfies `cond`; returns that frame. For a change that removes text
    /// from the screen, where only a complete frame can show the absence.
    fn send_and_wait_for_frame(
        &mut self,
        bytes: &[u8],
        what: &str,
        cond: impl Fn(&str) -> bool,
    ) -> String {
        let tail = self.send_and_wait_until(bytes, what, |text| {
            complete_frames(text).into_iter().any(&cond)
        });
        complete_frames(&tail)
            .into_iter()
            .find(|frame| cond(frame))
            .expect("the wait returned on this frame")
            .to_string()
    }

    /// Everything drawn so far, without escape sequences.
    fn text(&self) -> String {
        screen(&self.reader.output)
    }

    fn dashboard_contents(&self) -> String {
        fs::read_to_string(&self.dashboard_path).unwrap_or_default()
    }

    /// Presses `q`, reads the terminal to its end and waits for the exit.
    fn quit(&mut self) -> String {
        self.send(b"q");
        self.reader.read_to_end();
        let status = wait_child("fleet tui exit", &mut self.child.0);
        let cleaned = self.text();
        assert!(
            status.success(),
            "fleet tui exited with {status:?}:\n{cleaned}"
        );

        let python_hits = fs::read_to_string(&self.python_log).unwrap_or_default();
        assert!(
            python_hits.trim().is_empty(),
            "fleet tui touched python unexpectedly:\n{python_hits}"
        );
        cleaned
    }

    fn finish(mut self) -> String {
        let cleaned = self.quit();
        let send_hits = fs::read_to_string(&self.send_log).unwrap_or_default();
        assert!(
            send_hits.trim().is_empty(),
            "fleet tui sent tmux input unexpectedly:\n{send_hits}"
        );
        cleaned
    }

    fn finish_allowing_tmux_input(mut self) -> String {
        self.quit()
    }
}

/// The text after the last frame corner: the final frame drawn.
fn last_frame(cleaned: &str) -> String {
    let body = cleaned
        .rsplit_once('╔')
        .map(|(_, tail)| tail)
        .unwrap_or(cleaned);
    format!("╔{body}")
}

/// TC-08: `amplihack fleet tui` should handle a real PTY session without any
/// Python interpreter on the live path.
///
/// This is the closest thing to the manual QA-style probe inside the automated
/// suite: a fake `azlin` shell script serves fleet data while fake `python` and
/// `python3` binaries would leave a breadcrumb if the Rust binary ever tried to
/// invoke them.
///
/// Every step below waits for the screen text its key produces, never for a
/// length of time (issue #1977).
#[test]
fn tc08_fleet_tui_virtual_tty_stays_python_free() {
    let bin = amplihack_bin();
    require_binary!(bin);

    let mut probe = FleetTuiProbe::new(&bin, None);
    probe.wait_for_text("q quit");
    probe.send_and_wait_for(b"?", "KEYBINDING HELP");
    probe.send_and_wait_for_frame(b"?", "the help overlay to close", |frame| {
        !frame.contains("KEYBINDING HELP")
    });
    probe.send_and_wait_for(b"\x1b[C", "[detail]");

    let cleaned = probe.finish();
    assert!(
        cleaned.contains("KEYBINDING HELP"),
        "expected help overlay in PTY output:\n{cleaned}"
    );
    assert!(
        cleaned.contains("Session Detail") || cleaned.contains("[detail]"),
        "expected detail-tab output after right-arrow navigation:\n{cleaned}"
    );
}

/// TC-09: edited apply must stay Python-free and block dangerous input in the
/// virtual-TTY flow as well, not just in unit tests.
#[test]
fn tc09_fleet_tui_editor_apply_blocks_dangerous_input_without_python() {
    let bin = amplihack_bin();
    require_binary!(bin);

    let mut probe = FleetTuiProbe::new(&bin, None);
    probe.wait_for_text("q quit");
    probe.send_and_wait_for(b"d", "Prepared proposal for vm-1/claude-1:");
    probe.send_and_wait_for(b"e", "Loaded proposal into editor for vm-1/claude-1.");
    // The proposal is `wait`; four presses cycle it to `send_input`.
    probe.send_and_wait_for(b"t", "Editor action set to escalate");
    probe.send_and_wait_for(b"t", "Editor action set to mark_complete");
    probe.send_and_wait_for(b"t", "Editor action set to restart");
    probe.send_and_wait_for(b"t", "Editor action set to send_input");
    probe.send_and_wait_for(b"i", "Typing mode");
    probe.send_and_wait_for(b"rm -rf /", "rm -rf /");
    probe.send_and_wait_for(b"A", "dangerous-input policy");

    let cleaned = probe.finish();
    assert!(
        cleaned.contains("Action Editor")
            && cleaned.contains("Action: send_input")
            && cleaned.contains("Action choices")
            && cleaned.contains("> send_input")
            && cleaned.contains("Typing mode")
            && cleaned.contains("rm -rf /"),
        "expected proposal/editor output in PTY flow:\n{cleaned}"
    );
    assert!(
        cleaned.contains("Apply status") && cleaned.contains("dangerous-input policy"),
        "expected dangerous-input block in PTY output:\n{cleaned}"
    );
}

/// TC-10: the native TUI new-session flow should create a session over azlin
/// without touching Python or sending tmux input to an existing session.
///
/// Session creation is dispatched to a background thread (via `bg_tx`), and
/// the azlin shim writes `create_log` before that thread reports
/// `SessionCreated`. So once the TUI has drawn "Created session …" the log is
/// complete and is read once, without polling.
#[test]
fn tc10_fleet_tui_new_session_launches_without_python() {
    let bin = amplihack_bin();
    require_binary!(bin);

    let mut probe = FleetTuiProbe::new(&bin, None);
    // "q quit" is part of the controls line in every frame.
    probe.wait_for_text("q quit");
    probe.send_and_wait_for(b"n", "[new]");
    // The default agent is "claude"; one press makes it "copilot".
    probe.send_and_wait_for(b"t", "Agent type: copilot");
    probe.send_and_wait_for(b"\n", "Created session 'copilot-");
    let create_hits = fs::read_to_string(&probe.create_log).unwrap_or_default();

    let cleaned = probe.finish();
    assert!(
        cleaned.contains("New Session") && cleaned.contains("Agent type: copilot"),
        "expected new-session tab output in PTY flow:\n{cleaned}"
    );
    assert!(
        cleaned.contains("Created session 'copilot-"),
        "expected successful new-session status in PTY output:\n{cleaned}"
    );
    assert!(
        create_hits.contains("tmux new-session -d -s") && create_hits.contains("amplihack copilot"),
        "expected azlin create-session command in PTY flow, got:\n{create_hits}"
    );
}

/// TC-11: the native TUI adopt flow should add the selected live session to the
/// fleet queue without touching Python.
///
/// The queue is saved before the status line is set, so it is read once after
/// the status appears.
#[test]
fn tc11_fleet_tui_adopts_selected_session_without_python() {
    let bin = amplihack_bin();
    require_binary!(bin);

    let mut probe = FleetTuiProbe::new(&bin, None);
    probe.wait_for_text("q quit");
    probe.send_and_wait_for(b"A", "Adopted vm-1/claude-1 into the fleet queue.");
    let queue = fs::read_to_string(&probe.queue_path).unwrap_or_default();

    let cleaned = probe.finish();
    assert!(
        cleaned.contains("Adopted vm-1/claude-1 into the fleet queue."),
        "expected adopt success status in PTY output:\n{cleaned}"
    );
    assert!(
        queue.contains("\"assigned_vm\": \"vm-1\"")
            && queue.contains("\"assigned_session\": \"claude-1\""),
        "expected adopted task persisted to queue, got:\n{queue}"
    );
}

/// TC-12: the native TUI fleet tab should expose the Python-style managed/all
/// split without touching Python, including unmanaged sessions in the all view.
#[test]
fn tc12_fleet_tui_all_sessions_subview_stays_python_free() {
    let bin = amplihack_bin();
    require_binary!(bin);

    let mut probe = FleetTuiProbe::new_with_existing_vms(&bin, None, Some("vm-2"));
    probe.wait_for_text("q quit");
    probe.send_and_wait_for(b"t", "All Sessions");
    probe.send_and_wait_for(b"j", "Selected session: vm-2/copilot-9");

    let cleaned = probe.finish();
    assert!(
        cleaned.contains("All Sessions"),
        "expected all-sessions subview in PTY output:\n{cleaned}"
    );
    assert!(
        cleaned.contains("vm-2") && cleaned.contains("unmanaged"),
        "expected unmanaged session in PTY output:\n{cleaned}"
    );
}

/// TC-13: moving the fleet selection in the all-sessions view should update the
/// selected-session preview without touching Python.
#[test]
fn tc13_fleet_tui_selected_preview_tracks_visible_selection_without_python() {
    let bin = amplihack_bin();
    require_binary!(bin);

    let mut probe = FleetTuiProbe::new_with_existing_vms(&bin, None, Some("vm-2"));
    probe.wait_for_text("q quit");
    probe.send_and_wait_for(b"t", "All Sessions");
    probe.send_and_wait_for(b"j", "Selected session: vm-2/copilot-9");

    let cleaned = probe.finish();
    assert!(
        cleaned.contains("Selected session: vm-2/copilot-9"),
        "expected selected-session preview header in PTY output:\n{cleaned}"
    );
    assert!(
        cleaned.contains("branch: side-quest")
            && cleaned.contains("repo: https://github.com/org/excluded.git")
            && cleaned.contains("cwd: /tmp/excluded")
            && cleaned.contains("Working outside fleet"),
        "expected selected-session preview body in PTY output:\n{cleaned}"
    );
}

/// TC-14: the native detail tab should surface the selected session's metadata
/// from native discovery without touching Python.
#[test]
fn tc14_fleet_tui_detail_view_shows_session_metadata_without_python() {
    let bin = amplihack_bin();
    require_binary!(bin);

    let mut probe = FleetTuiProbe::new(&bin, None);
    probe.wait_for_text("q quit");
    probe.send_and_wait_for(b"\x1b[C", "Session Detail");

    let cleaned = probe.finish();
    assert!(
        cleaned.contains("Session Detail"),
        "expected detail tab in PTY output:\n{cleaned}"
    );
    assert!(
        cleaned.contains("branch: main")
            && cleaned.contains("repo: https://github.com/org/demo.git")
            && cleaned.contains("cwd: /tmp/demo")
            && cleaned.contains("task: Awaiting operator confirmation"),
        "expected discovered session metadata in detail PTY output:\n{cleaned}"
    );
}

/// TC-15: the native projects tab should add and remove projects interactively
/// without touching Python.
#[test]
fn tc15_fleet_tui_projects_tab_adds_and_removes_projects_without_python() {
    let bin = amplihack_bin();
    require_binary!(bin);

    let mut probe = FleetTuiProbe::new(&bin, None);
    probe.wait_for_text("q quit");
    probe.send_and_wait_for(b"p", "No projects registered.");
    // `i` opens the inline repo prompt; the keys after it are read by that
    // prompt in order, so they go in one write.
    probe.send_and_wait_for(
        b"ihttps://github.com/org/new-repo\n",
        "Added project 'new-repo' to the dashboard.",
    );
    probe.send_and_wait_for(b"x", "Removed project 'new-repo' from the dashboard.");

    let dashboard = probe.dashboard_contents();
    let cleaned = probe.finish();
    assert!(
        cleaned.contains("Removed project 'new-repo' from the dashboard."),
        "expected project removal status in PTY output:\n{cleaned}"
    );
    assert!(
        !dashboard.contains("new-repo"),
        "expected removed project to be absent from dashboard file:\n{dashboard}"
    );
}

/// TC-16: the native dry-run path should render a proposal in the detail view
/// without touching Python.
#[test]
fn tc16_fleet_tui_dry_run_shows_proposal_without_python() {
    let bin = amplihack_bin();
    require_binary!(bin);

    let mut probe = FleetTuiProbe::new(&bin, None);
    probe.wait_for_text("q quit");
    probe.send_and_wait_for(b"d", "Prepared proposal for vm-1/claude-1:");

    let cleaned = probe.finish();
    assert!(
        cleaned.contains("Prepared proposal for vm-1/claude-1:"),
        "expected dry-run status in PTY output:\n{cleaned}"
    );
    assert!(
        cleaned.contains("Prepared proposal")
            && cleaned.contains("Session Detail")
            && cleaned.contains("Action:")
            && cleaned.contains("Confidence:")
            && cleaned.contains("Reasoning:"),
        "expected proposal detail in PTY output:\n{cleaned}"
    );
}

/// TC-17: numeric tab hotkeys should follow the Python dashboard order without
/// touching Python: `3` opens the editor and `4` opens projects.
#[test]
fn tc17_fleet_tui_numeric_tab_hotkeys_follow_python_order_without_python() {
    let bin = amplihack_bin();
    require_binary!(bin);

    let mut probe = FleetTuiProbe::new(&bin, None);
    probe.wait_for_text("q quit");
    probe.send_and_wait_for(b"3", "[editor]");
    probe.send_and_wait_for(b"4", "No projects registered.");

    let cleaned = probe.finish();
    assert!(
        cleaned.contains("Action Editor"),
        "expected editor tab after pressing 3:\n{cleaned}"
    );
    assert!(
        cleaned.contains("[projects]") && cleaned.contains("No projects registered."),
        "expected projects tab after pressing 4:\n{cleaned}"
    );
}

/// TC-24: canceling out of the focused multiline editor should return to Detail
/// without applying changes or touching Python.
#[test]
fn tc24_fleet_tui_editor_cancel_returns_to_detail_without_python() {
    let bin = amplihack_bin();
    require_binary!(bin);

    let mut probe = FleetTuiProbe::new(&bin, None);
    probe.wait_for_text("q quit");
    probe.send_and_wait_for(b"d", "Prepared proposal for vm-1/claude-1:");
    probe.send_and_wait_for(b"e", "Loaded proposal into editor for vm-1/claude-1.");
    probe.send_and_wait_for(b"i", "Typing mode");
    probe.send_and_wait_for(b"scratch", "scratch");
    // A lone Esc: nothing is sent after it until its effect is drawn, so the
    // TUI cannot read it as the start of an escape sequence.
    probe.send_and_wait_for(b"\x1b", "Editor changes discarded.");

    let cleaned = probe.finish();
    assert!(
        cleaned.contains("Editor changes discarded."),
        "expected cancel status in PTY output:\n{cleaned}"
    );
    assert!(
        cleaned.contains("Session Detail"),
        "expected detail tab after cancel in PTY output:\n{cleaned}"
    );
}

/// TC-18: entering the detail tab should fetch a fresh tmux capture immediately
/// instead of only showing the observer summary lines.
#[test]
fn tc18_fleet_tui_detail_tab_fetches_fresh_tmux_capture_without_python() {
    let bin = amplihack_bin();
    require_binary!(bin);

    let mut probe = FleetTuiProbe::new(&bin, None);
    probe.wait_for_text("q quit");
    probe.send_and_wait_for(b"2", "FULL DETAIL: deployment checklist open");

    let cleaned = probe.finish();
    assert!(
        cleaned.contains("Session Detail"),
        "expected detail tab in PTY output:\n{cleaned}"
    );
    assert!(
        cleaned.contains("FULL DETAIL: deployment checklist open")
            && cleaned.contains("Line 2: awaiting explicit operator input"),
        "expected fresh tmux capture in detail PTY output:\n{cleaned}"
    );
}

/// TC-19: the Python dashboard's fleet logo should render by default and hide
/// after pressing `l`, without any Python fallback.
#[test]
fn tc19_fleet_tui_logo_toggle_hides_logo_without_python() {
    let bin = amplihack_bin();
    require_binary!(bin);

    let mut probe = FleetTuiProbe::new(&bin, None);
    probe.wait_for_text("A M P L I H A C K   F L E E T");
    probe.send_and_wait_for_frame(b"l", "a frame without the logo", |frame| {
        !frame.contains("A M P L I H A C K   F L E E T")
    });

    let cleaned = probe.finish();
    assert!(
        cleaned.contains("A M P L I H A C K   F L E E T"),
        "expected logo in initial PTY output:\n{cleaned}"
    );
    let last_frame = last_frame(&cleaned);
    assert!(
        !last_frame.contains("A M P L I H A C K   F L E E T") && !last_frame.contains("|  ☠  |"),
        "expected logo to be hidden after pressing l:\n{last_frame}"
    );
}

/// TC-20: the native fleet tab should support inline session search in a real
/// PTY without touching Python.
#[test]
fn tc20_fleet_tui_session_search_filters_by_vm_without_python() {
    let bin = amplihack_bin();
    require_binary!(bin);

    let mut probe = FleetTuiProbe::new_with_existing_vms(&bin, None, Some("vm-2"));
    probe.wait_for_text("q quit");
    probe.send_and_wait_for(b"t", "All Sessions");
    // `/` opens the inline search, which reads the keys after it in order.
    probe.send_and_wait_for(b"/vm-2\n", "Selected session: vm-2/copilot-9");

    let cleaned = probe.finish();
    assert!(
        cleaned.contains("Search: vm-2 (press / to edit, Esc to clear)")
            || cleaned.contains("search: vm-2"),
        "expected active fleet search in PTY output:\n{cleaned}"
    );
    assert!(
        cleaned.contains("Selected session: vm-2/copilot-9"),
        "expected search-filtered selected preview in PTY output:\n{cleaned}"
    );
}

/// TC-21: the native fleet TUI should stream partial refresh progress instead of
/// waiting for every VM poll to finish before the first repaint.
///
/// The frame drawn while vm-2 is being polled must already show vm-1's
/// session. That frame is captured whatever vm-2's poll costs, so the fixture
/// no longer slows vm-2 down.
#[test]
fn tc21_fleet_tui_refresh_streams_partial_results_without_python() {
    let bin = amplihack_bin();
    require_binary!(bin);

    let mut probe = FleetTuiProbe::new_with_existing_vms(&bin, None, Some("vm-2"));
    let partial = probe.wait_since(0, "a frame polling vm-2", |text| {
        complete_frames(text)
            .into_iter()
            .any(|frame| frame.contains("refresh: 1/2 polling vm-2"))
    });
    let partial = complete_frames(&partial)
        .into_iter()
        .find(|frame| frame.contains("refresh: 1/2 polling vm-2"))
        .expect("the wait returned on this frame")
        .to_string();
    assert!(
        partial.contains("vm-1") && partial.contains("claude-1"),
        "expected first VM to render before the second VM completed:\n{partial}"
    );
    probe.wait_for_text("refresh: 2/2 complete");

    let cleaned = probe.finish();
    assert!(
        cleaned.contains("refresh: 2/2 complete") || cleaned.contains("claude-1"),
        "expected the refresh to complete cleanly after the streamed partial output:\n{cleaned}"
    );
}

/// TC-22: skipping a prepared proposal should leave visible proposal feedback in
/// the detail pane without touching Python.
#[test]
fn tc22_fleet_tui_skip_keeps_detail_feedback_without_python() {
    let bin = amplihack_bin();
    require_binary!(bin);

    let mut probe = FleetTuiProbe::new(&bin, None);
    probe.wait_for_text("q quit");
    probe.send_and_wait_for(b"d", "Prepared proposal for vm-1/claude-1:");
    probe.send_and_wait_until(b"x", "the skip notice", |text| {
        text.contains("Proposal status") && text.contains("Skipped.")
    });

    let cleaned = probe.finish();
    assert!(
        cleaned.contains("Proposal status") && cleaned.contains("Skipped."),
        "expected persisted skip feedback in detail output:\n{cleaned}"
    );
}

/// TC-23: a native reasoner failure should stay in the dashboard and surface a
/// visible notice instead of degrading silently.
#[test]
fn tc23_fleet_tui_reasoner_failure_is_visible_without_python() {
    let bin = amplihack_bin();
    require_binary!(bin);

    let mut probe = FleetTuiProbe::new_with_reasoner_script(
        &bin,
        "#!/bin/sh\necho 'ANTHROPIC_API_KEY missing' >&2\nexit 1\n",
    );
    probe.wait_for_text("q quit");
    probe.send_and_wait_until(b"d", "the reasoner failure notice", |text| {
        text.contains("Reasoner status")
            && text.contains("ANTHROPIC_API_KEY missing")
            && text.contains("heuristic proposal")
    });

    let cleaned = probe.finish();
    assert!(
        cleaned.contains("Reasoner status")
            && cleaned.contains("ANTHROPIC_API_KEY missing")
            && cleaned.contains("heuristic proposal"),
        "expected visible reasoner failure notice in detail output:\n{cleaned}"
    );
}

/// TC-25: the native dashboard should match the Python fleet scenario where
/// Enter opens the selected session's detail view and Esc returns to Fleet,
/// without touching Python.
#[test]
fn tc25_fleet_tui_enter_opens_detail_and_escape_returns_to_fleet_without_python() {
    let bin = amplihack_bin();
    require_binary!(bin);

    let mut probe = FleetTuiProbe::new(&bin, None);
    probe.wait_for_text("q quit");
    probe.send_and_wait_until(b"\n", "the detail actions", |text| {
        text.contains("Detail actions") && text.contains("d prepare proposal")
    });
    probe.send_and_wait_for(b"\x1b", "[fleet]");

    let cleaned = probe.finish();
    assert!(
        cleaned.contains("Session Detail"),
        "expected detail tab after pressing Enter in PTY output:\n{cleaned}"
    );
    assert!(
        cleaned.contains("Detail actions") && cleaned.contains("d prepare proposal"),
        "expected visible detail action hints after pressing Enter in PTY output:\n{cleaned}"
    );
    let last_frame = last_frame(&cleaned);
    assert!(
        last_frame.contains("[fleet]"),
        "expected fleet tab after pressing Esc in PTY output:\n{last_frame}"
    );
}

/// TC-26: the native dashboard should match the Python force-refresh scenario:
/// pressing `r` should kick off a fresh refresh cycle that becomes visible in
/// the PTY output without touching Python.
#[test]
fn tc26_fleet_tui_force_refresh_restarts_visible_polling_without_python() {
    let bin = amplihack_bin();
    require_binary!(bin);

    let mut probe = FleetTuiProbe::new_with_existing_vms(&bin, None, Some("vm-2"));
    // The initial refresh settles before the force-refresh.
    probe.wait_for_text("refresh: 2/2 complete");

    let polling = probe.send_and_wait_for(b"r", "refresh: 1/2 polling vm-2");
    let mark = probe.mark();
    probe.wait_since(mark, "refresh: 2/2 complete", |text| {
        text.contains("refresh: 2/2 complete")
    });

    let cleaned = probe.finish();
    assert!(
        polling.contains("refresh: 1/2 polling vm-2"),
        "expected fresh in-flight refresh progress after pressing r:\n{cleaned}"
    );
    assert!(
        cleaned.contains("refresh: 2/2 complete"),
        "expected force-refresh cycle to complete after pressing r:\n{cleaned}"
    );
}

/// TC-27: a successful native apply should stay Python-free, surface visible
/// success feedback, and send the expected tmux input through azlin.
///
/// The azlin shim logs the send-keys call before `apply` returns and sets its
/// status, so the log is read once after the status appears.
#[test]
fn tc27_fleet_tui_apply_prepared_proposal_sends_tmux_input_without_python() {
    let bin = amplihack_bin();
    require_binary!(bin);

    let mut probe = FleetTuiProbe::new(
        &bin,
        Some(
            r#"{"action":"send_input","confidence":0.85,"reasoning":"Need operator confirmation","input_text":"y"}"#,
        ),
    );
    probe.wait_for_text("q quit");
    probe.send_and_wait_for(b"d", "Prepared proposal for vm-1/claude-1:");
    probe.send_and_wait_for(b"a", "Applied send_input to vm-1/claude-1.");
    let send_hits = fs::read_to_string(&probe.send_log).unwrap_or_default();

    let cleaned = probe.finish_allowing_tmux_input();
    assert!(
        cleaned.contains("Apply status")
            && cleaned.contains("Applied send_input to vm-1/claude-1."),
        "expected persistent apply success notice in PTY output:\n{cleaned}"
    );
    assert!(
        send_hits.contains("tmux send-keys -t 'claude-1' 'y' Enter"),
        "expected azlin tmux send-keys command for apply flow, got:\n{send_hits}"
    );
}

/// TC-28: the Python dashboard's letter hotkeys should also work in the native
/// raw-terminal TUI without touching Python: `p` opens Projects, `f` returns to
/// Fleet, and `s` opens Session Detail.
#[test]
fn tc28_fleet_tui_letter_tab_hotkeys_follow_python_order_without_python() {
    let bin = amplihack_bin();
    require_binary!(bin);

    let mut probe = FleetTuiProbe::new(&bin, None);
    probe.wait_for_text("q quit");
    probe.send_and_wait_for(b"p", "No projects registered.");
    probe.send_and_wait_for(b"f", "[fleet]");
    probe.send_and_wait_for(b"s", "Session Detail");

    let cleaned = probe.finish();
    assert!(
        cleaned.contains("[projects]") && cleaned.contains("No projects registered."),
        "expected projects tab after pressing p:\n{cleaned}"
    );
    assert!(
        cleaned.contains("[fleet]"),
        "expected fleet tab after pressing f:\n{cleaned}"
    );
    assert!(
        cleaned.contains("Session Detail"),
        "expected detail tab after pressing s:\n{cleaned}"
    );
}

/// TC-29: a successful edited apply should stay Python-free, surface visible
/// success feedback, and send the expected tmux input via the editor-tab `A`
/// path rather than only through prepared-apply.
#[test]
fn tc29_fleet_tui_apply_edited_proposal_sends_tmux_input_without_python() {
    let bin = amplihack_bin();
    require_binary!(bin);

    let mut probe = FleetTuiProbe::new(
        &bin,
        Some(
            r#"{"action":"send_input","confidence":0.85,"reasoning":"Need operator confirmation","input_text":"y"}"#,
        ),
    );
    probe.wait_for_text("q quit");
    probe.send_and_wait_for(b"d", "Prepared proposal for vm-1/claude-1:");
    probe.send_and_wait_for(b"e", "Loaded proposal into editor for vm-1/claude-1.");
    probe.send_and_wait_for(b"A", "Applied edited send_input to vm-1/claude-1.");
    let send_hits = fs::read_to_string(&probe.send_log).unwrap_or_default();

    let cleaned = probe.finish_allowing_tmux_input();
    assert!(
        cleaned.contains("Apply status")
            && cleaned.contains("Applied edited send_input to vm-1/claude-1."),
        "expected persistent edited-apply success notice in PTY output:\n{cleaned}"
    );
    assert!(
        send_hits.contains("tmux send-keys -t 'claude-1' 'y' Enter"),
        "expected azlin tmux send-keys command for edited apply flow, got:\n{send_hits}"
    );
}

/// TC-30: raw left/right arrow tab navigation should follow the same wrap order
/// as the Python dashboard, not just the letter/numeric hotkeys.
#[test]
fn tc30_fleet_tui_arrow_tab_navigation_wraps_without_python() {
    let bin = amplihack_bin();
    require_binary!(bin);

    let mut probe = FleetTuiProbe::new(&bin, None);
    probe.wait_for_text("q quit");

    // Each wait looks only at what is drawn after its key, and names the
    // active tab, so every step proves its own move.
    probe.send_and_wait_for(b"\x1b[D", "[new]");
    probe.send_and_wait_for(b"\x1b[C", "[fleet]");
    probe.send_and_wait_for(b"\x1b[C", "[detail]");
    probe.send_and_wait_for(b"\x1b[C", "[editor]");
    probe.send_and_wait_for(b"\x1b[C", "[projects]");
    // The body follows the tab bar, so it is checked in a complete frame.
    let wrapped = probe.send_and_wait_for_frame(b"\x1b[C", "a frame on the New tab", |frame| {
        frame.contains("[new]")
    });

    probe.finish();
    assert!(
        wrapped.contains("New Session"),
        "expected final wrapped arrow traversal to land on New Session:\n{wrapped}"
    );
}

/// TC-31: pressing Enter after filtering the fleet list down to zero visible
/// sessions should still stay in the native TUI, switch to Detail, and show the
/// explicit no-selection state instead of falling through silently.
#[test]
fn tc31_fleet_tui_enter_with_no_visible_selection_shows_detail_notice_without_python() {
    let bin = amplihack_bin();
    require_binary!(bin);

    let mut probe = FleetTuiProbe::new_with_existing_vms(&bin, None, Some("vm-2"));
    probe.wait_for_text("q quit");
    probe.send_and_wait_for(
        b"/missing-session\n",
        "No sessions match the current search. Press Esc to clear.",
    );
    probe.send_and_wait_for(b"\n", "No session selected.");

    let cleaned = probe.finish();
    assert!(
        cleaned.contains("Search: missing-session (press / to edit, Esc to clear)")
            || cleaned.contains("search: missing-session"),
        "expected active empty-result search in PTY output:\n{cleaned}"
    );
    assert!(
        cleaned.contains("No sessions match the current search. Press Esc to clear."),
        "expected explicit empty-result fleet message in PTY output:\n{cleaned}"
    );
    assert!(
        cleaned.contains("No session selected."),
        "expected detail-tab no-selection message after pressing Enter:\n{cleaned}"
    );
}

/// TC-32: raw up/down cursor navigation on the Fleet session table should
/// update the selected-session preview, matching the Python outside-in session
/// table cursor navigation contract without touching Python.
#[test]
fn tc32_fleet_tui_up_down_cursor_navigation_updates_preview_without_python() {
    let bin = amplihack_bin();
    require_binary!(bin);

    let mut probe = FleetTuiProbe::new_with_existing_vms(&bin, None, Some("vm-2"));
    probe.wait_for_text("Selected session: vm-1/claude-1");

    probe.send_and_wait_for(b"t", "All Sessions");
    // The preview lines follow the selection line, so each move is checked
    // in a complete frame.
    let down_frame = probe.send_and_wait_for_frame(b"\x1b[B", "vm-2/copilot-9 selected", |frame| {
        frame.contains("Selected session: vm-2/copilot-9")
    });
    assert!(
        down_frame.contains("Working outside fleet")
            && down_frame.contains("branch: side-quest")
            && down_frame.contains("repo: https://github.com/org/excluded.git"),
        "expected down-arrow preview update for vm-2/copilot-9:\n{down_frame}"
    );

    let up_frame = probe.send_and_wait_for_frame(b"\x1b[A", "vm-1/claude-1 selected", |frame| {
        frame.contains("Selected session: vm-1/claude-1")
    });

    let cleaned = probe.finish();
    assert!(
        up_frame.contains("Awaiting operator confirmation")
            && up_frame.contains("branch: main")
            && up_frame.contains("repo: https://github.com/org/demo.git"),
        "expected up-arrow preview update for vm-1/claude-1:\n{up_frame}"
    );
    assert!(
        cleaned.contains("Selected session: vm-1/claude-1"),
        "expected final PTY output to end back on vm-1/claude-1:\n{cleaned}"
    );
}

/// TC-33: a stale prepared proposal must not apply after the current fleet
/// selection disappears; the native TUI should surface an explicit no-selection
/// notice instead of sending tmux input.
#[test]
fn tc33_fleet_tui_stale_proposal_does_not_apply_when_selection_disappears_without_python() {
    let bin = amplihack_bin();
    require_binary!(bin);

    let mut probe = FleetTuiProbe::new_with_existing_vms(&bin, None, Some("vm-2"));
    probe.wait_for_text("q quit");
    // The dry-run moves to the detail tab; Esc returns to the fleet tab.
    probe.send_and_wait_for(b"d", "Prepared proposal for vm-1/claude-1:");
    probe.send_and_wait_for(b"\x1b", "[fleet]");
    probe.send_and_wait_for(
        b"/missing-session\n",
        "No sessions match the current search. Press Esc to clear.",
    );
    probe.send_and_wait_for(b"a", "No session selected to apply.");

    let cleaned = probe.finish();
    assert!(
        cleaned.contains("Prepared proposal for vm-1/claude-1:")
            && cleaned.contains("No session selected to apply."),
        "expected stale-proposal apply guard in PTY output:\n{cleaned}"
    );
}

/// TC-34: pressing `e` without a prepared proposal should stay on the current
/// fleet view and surface the explicit no-proposal notice rather than switching
/// into the editor.
#[test]
fn tc34_fleet_tui_edit_without_proposal_stays_out_of_editor_without_python() {
    let bin = amplihack_bin();
    require_binary!(bin);

    let mut probe = FleetTuiProbe::new(&bin, None);
    probe.wait_for_text("q quit");
    probe.send_and_wait_for(b"e", "No prepared proposal for the selected session.");

    let cleaned = probe.finish();
    let last_frame = last_frame(&cleaned);
    assert!(
        last_frame.contains("[fleet]"),
        "expected to remain on the fleet tab after pressing e without a proposal:\n{last_frame}"
    );
    assert!(
        cleaned.contains("No prepared proposal for the selected session.")
            && !cleaned.contains("Loaded proposal into editor for"),
        "expected explicit no-proposal notice without editor activation:\n{cleaned}"
    );
}

/// TC-35: a running VM with no tmux sessions should still render the explicit
/// placeholder row in the live raw-terminal fleet view without touching Python.
#[test]
fn tc35_fleet_tui_running_vm_with_no_sessions_shows_placeholder_without_python() {
    let bin = amplihack_bin();
    require_binary!(bin);

    let azlin_script = r#"#!/bin/sh
if [ "$1" = "list" ] && [ "$2" = "--json" ]; then
  printf '%s\n' '[{"name":"empty-vm","status":"Running","region":"westus","session_name":"empty-vm"}]'
  exit 0
fi
if [ "$1" = "connect" ] && [ "$2" = "empty-vm" ]; then
  case "$*" in
    *"tmux list-sessions"*)
      exit 0
      ;;
  esac
fi
exit 1
"#;

    let mut probe = FleetTuiProbe::new_with_custom_azlin_script(&bin, None, azlin_script);
    probe.wait_for_text("empty-vm/(no sessions)");

    let cleaned = probe.finish();
    assert!(
        cleaned.contains("empty-vm/(no sessions)") && cleaned.contains("(empty)"),
        "expected running-empty-vm placeholder row in PTY output:\n{cleaned}"
    );
}

/// TC-36: pressing `a` when the fleet view has no current selection should stay
/// native and surface the explicit apply warning instead of crashing or silently
/// doing nothing.
#[test]
fn tc36_fleet_tui_apply_without_selection_shows_warning_without_python() {
    let bin = amplihack_bin();
    require_binary!(bin);

    let mut probe = FleetTuiProbe::new_with_existing_vms(&bin, None, Some("vm-2"));
    probe.wait_for_text("q quit");
    probe.send_and_wait_for(
        b"/missing-session\n",
        "No sessions match the current search. Press Esc to clear.",
    );
    probe.send_and_wait_for(b"a", "No session selected to apply.");

    let cleaned = probe.finish();
    assert!(
        cleaned.contains("No sessions match the current search. Press Esc to clear."),
        "expected explicit empty-result fleet message in PTY output:\n{cleaned}"
    );
    assert!(
        cleaned.contains("No session selected to apply."),
        "expected visible apply warning in PTY output:\n{cleaned}"
    );
}

/// TC-37: the live raw-terminal fleet view should render the visible summary bar
/// with the same operator-facing status buckets that Python exposes.
#[test]
fn tc37_fleet_tui_shows_status_summary_bar_without_python() {
    let bin = amplihack_bin();
    require_binary!(bin);

    let mut probe = FleetTuiProbe::new_with_existing_vms(&bin, None, Some("vm-2"));
    probe.wait_for_text("active:");

    let cleaned = probe.finish();
    assert!(
        cleaned.contains("active:")
            && cleaned.contains("idle:")
            && cleaned.contains("waiting:")
            && cleaned.contains("error:"),
        "expected visible fleet status summary bar in PTY output:\n{cleaned}"
    );
}

/// TC-38: the live raw-terminal fleet view should render the visible footer
/// control strip so the operator can discover keybindings without Python.
#[test]
fn tc38_fleet_tui_shows_footer_keybindings_without_python() {
    let bin = amplihack_bin();
    require_binary!(bin);

    let mut probe = FleetTuiProbe::new(&bin, None);
    probe.wait_for_text("q quit");

    let cleaned = probe.finish();
    assert!(
        cleaned.contains("q quit") && cleaned.contains("r refresh") && cleaned.contains("? help"),
        "expected visible fleet footer keybindings in PTY output:\n{cleaned}"
    );
}

/// TC-39: the live raw-terminal fleet dashboard should render its default
/// managed-subview chrome so operators can tell which fleet slice they are in
/// without relying on Python/Textual scaffolding.
#[test]
fn tc39_fleet_tui_shows_managed_subview_header_without_python() {
    let bin = amplihack_bin();
    require_binary!(bin);

    let mut probe = FleetTuiProbe::new(&bin, None);
    probe.wait_for_text("Managed Sessions");

    let cleaned = probe.finish();
    assert!(
        cleaned.contains("[managed]") && cleaned.contains("Managed Sessions"),
        "expected visible managed fleet subview header in PTY output:\n{cleaned}"
    );
}
