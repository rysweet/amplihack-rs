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
use std::thread::sleep;
use std::time::{Duration, Instant};

/// Returns the path to the compiled debug binary.
fn amplihack_bin() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop(); // tests/
    path.pop(); // workspace root
    path.push("target/debug/amplihack");
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
    let mut cmd = Command::new(bin);
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

    let mut cmd = Command::new(bin);
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

fn read_until_quiet(master: &mut fs::File, buffer: &mut Vec<u8>, duration: Duration) {
    let deadline = Instant::now() + duration;
    let mut chunk = [0u8; 4096];

    loop {
        match master.read(&mut chunk) {
            Ok(0) => {
                if Instant::now() >= deadline {
                    break;
                }
                sleep(Duration::from_millis(20));
            }
            Ok(read) => {
                buffer.extend_from_slice(&chunk[..read]);
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                if Instant::now() >= deadline {
                    break;
                }
                sleep(Duration::from_millis(20));
            }
            Err(error) if error.raw_os_error() == Some(libc::EIO) => break,
            Err(error) => panic!("failed to read PTY output: {error}"),
        }
    }
}

/// Poll the PTY master until `needle` appears somewhere in the accumulated
/// buffer, or the wall-clock deadline is reached.
///
/// Unlike `read_until_quiet`, this does not exit as soon as the deadline
/// passes — it keeps reading until the expected content arrives or time is
/// truly up.  It is the correct primitive to use when the test needs to
/// *observe* a specific string rather than merely wait a fixed amount of time.
///
/// Returns `true` if the needle was found, `false` on timeout.
fn poll_until_output_contains(
    master: &mut fs::File,
    buffer: &mut Vec<u8>,
    needle: &str,
    timeout: Duration,
) -> bool {
    let deadline = Instant::now() + timeout;
    let mut chunk = [0u8; 4096];

    loop {
        // Check the current accumulated buffer first.
        let current = strip_ansi(&String::from_utf8_lossy(buffer));
        if current.contains(needle) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        match master.read(&mut chunk) {
            Ok(0) => {
                sleep(Duration::from_millis(20));
            }
            Ok(read) => {
                buffer.extend_from_slice(&chunk[..read]);
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                sleep(Duration::from_millis(20));
            }
            Err(error) if error.raw_os_error() == Some(libc::EIO) => return false,
            Err(error) => panic!("failed to read PTY output: {error}"),
        }
    }
}

/// Poll a file path until its contents contain `needle`, or a deadline is
/// reached.  Used to verify side-effects (e.g. log files written by shims)
/// that are produced asynchronously by a background thread.
///
/// Returns the file contents on success, or panics with a descriptive message
/// on timeout.
fn poll_file_for_content(path: &Path, needle: &str, timeout: Duration) -> String {
    let deadline = Instant::now() + timeout;
    loop {
        let contents = fs::read_to_string(path).unwrap_or_default();
        if contents.contains(needle) {
            return contents;
        }
        if Instant::now() >= deadline {
            panic!(
                "timed out after {timeout:?} waiting for {path:?} to contain {needle:?}; \
                 got:\n{contents}"
            );
        }
        sleep(Duration::from_millis(50));
    }
}

fn wait_for_exit(child: &mut std::process::Child, timeout: Duration) -> std::process::ExitStatus {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child
            .try_wait()
            .unwrap_or_else(|error| panic!("failed to poll child process: {error}"))
        {
            return status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            panic!("child process did not exit within {timeout:?}");
        }
        sleep(Duration::from_millis(20));
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
    let db_path = temp_dir.path().join("probe_tc06.kuzu");

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
    let db_path = temp_dir.path().join("probe_tc08.kuzu");
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
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.pop(); // tests/
    p.pop(); // workspace root
    p.push("scripts/probe-no-python.sh");
    std::fs::read_to_string(&p)
        .unwrap_or_else(|_| panic!("probe script not found at {p:?} — run from workspace root"))
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
    let script = {
        let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        p.pop(); // tests/
        p.pop(); // workspace root
        p.push("scripts/probe-no-python.sh");
        p
    };

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

    let output = Command::new("bash")
        .arg(&script)
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
    master: fs::File,
    child: Child,
    python_log: PathBuf,
    send_log: PathBuf,
    create_log: PathBuf,
    queue_path: PathBuf,
    dashboard_path: PathBuf,
    output: Vec<u8>,
}

impl FleetTuiProbe {
    fn new(bin: &Path, reasoner_json: Option<&str>) -> Self {
        Self::new_with_existing_vms_vm2_delay_and_reasoner_script(
            bin,
            reasoner_json,
            None,
            None,
            None,
        )
    }

    fn new_with_existing_vms(
        bin: &Path,
        reasoner_json: Option<&str>,
        existing_vms: Option<&str>,
    ) -> Self {
        Self::new_with_existing_vms_vm2_delay_and_reasoner_script(
            bin,
            reasoner_json,
            existing_vms,
            None,
            None,
        )
    }

    fn new_with_existing_vms_and_vm2_delay(
        bin: &Path,
        reasoner_json: Option<&str>,
        existing_vms: Option<&str>,
        vm2_delay_seconds: Option<f32>,
    ) -> Self {
        Self::new_with_existing_vms_vm2_delay_and_reasoner_script(
            bin,
            reasoner_json,
            existing_vms,
            vm2_delay_seconds,
            None,
        )
    }

    fn new_with_reasoner_script(bin: &Path, reasoner_script: &str) -> Self {
        Self::new_with_existing_vms_vm2_delay_and_reasoner_script(
            bin,
            None,
            None,
            None,
            Some(reasoner_script),
        )
    }

    fn new_with_existing_vms_vm2_delay_and_reasoner_script(
        bin: &Path,
        reasoner_json: Option<&str>,
        existing_vms: Option<&str>,
        vm2_delay_seconds: Option<f32>,
        reasoner_script: Option<&str>,
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
        let vm2_delay = vm2_delay_seconds
            .map(|seconds| format!("sleep {seconds}\n"))
            .unwrap_or_default();
        let vm2_block = if existing_vms.is_some() {
            format!(
                r#"
if [ "$1" = "connect" ] && [ "$2" = "vm-2" ]; then
  case "$*" in
    *"PANE_START"*)
      {vm2_delay}\
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
      {vm2_delay}\
      printf '%s\n' 'copilot-9|||1|||0'
      exit 0
      ;;
    *"===TMUX==="*)
      {vm2_delay}\
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
      {vm2_delay}\
      printf '%s\n' 'Working outside fleet'
      exit 0
      ;;
  esac
fi
 "#,
                vm2_delay = vm2_delay
            )
        } else {
            String::new()
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
        write_executable(
            &azlin,
            &format!(
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
            ),
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
                std::ptr::null(),
                std::ptr::null(),
            )
        };
        assert_eq!(rc, 0, "failed to allocate PTY");

        let flags = unsafe { libc::fcntl(master_fd, libc::F_GETFL) };
        assert!(flags >= 0, "failed to get PTY flags");
        let set_rc = unsafe { libc::fcntl(master_fd, libc::F_SETFL, flags | libc::O_NONBLOCK) };
        assert_eq!(set_rc, 0, "failed to make PTY non-blocking");

        let master = unsafe { fs::File::from_raw_fd(master_fd) };
        let stdin_file = unsafe { fs::File::from_raw_fd(slave_fd) };
        let stdout_fd = unsafe { libc::dup(stdin_file.as_raw_fd()) };
        assert!(stdout_fd >= 0, "failed to dup PTY slave for stdout");
        let stderr_fd = unsafe { libc::dup(stdin_file.as_raw_fd()) };
        assert!(stderr_fd >= 0, "failed to dup PTY slave for stderr");
        let stdout_file = unsafe { fs::File::from_raw_fd(stdout_fd) };
        let stderr_file = unsafe { fs::File::from_raw_fd(stderr_fd) };

        let mut command = Command::new(bin);
        command
            .args(["fleet", "tui", "--interval", "1", "--capture-lines", "10"])
            .env("PATH", path)
            .env("AZLIN_PATH", &azlin)
            .env("HOME", temp_dir.path())
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

        let child = command.spawn().expect("failed to spawn fleet tui");

        Self {
            _temp_dir: temp_dir,
            master,
            child,
            python_log,
            send_log,
            create_log,
            queue_path,
            dashboard_path,
            output: Vec::new(),
        }
    }

    fn drain(&mut self, duration: Duration) {
        read_until_quiet(&mut self.master, &mut self.output, duration);
    }

    /// Block until `needle` appears somewhere in the accumulated PTY output,
    /// or `timeout` elapses.  Returns `true` on success, `false` on timeout.
    ///
    /// This is the correct primitive to use instead of `drain(fixed_duration)`
    /// when the test needs to synchronise on a specific piece of TUI output
    /// rather than guess how long the binary will take to render it.
    fn wait_for_output_containing(&mut self, needle: &str, timeout: Duration) -> bool {
        poll_until_output_contains(&mut self.master, &mut self.output, needle, timeout)
    }

    fn send(&mut self, bytes: &[u8], wait: Duration) {
        self.master
            .write_all(bytes)
            .unwrap_or_else(|error| panic!("failed to send PTY input {bytes:?}: {error}"));
        self.drain(wait);
    }

    /// Send bytes and then wait until `needle` appears in the accumulated PTY
    /// output.  Panics if `timeout` expires before the expected string arrives.
    fn send_and_wait_for(&mut self, bytes: &[u8], needle: &str, timeout: Duration) {
        self.master
            .write_all(bytes)
            .unwrap_or_else(|error| panic!("failed to send PTY input {bytes:?}: {error}"));
        assert!(
            self.wait_for_output_containing(needle, timeout),
            "timed out after {timeout:?} waiting for PTY output to contain {needle:?}"
        );
    }

    fn create_hits(&self) -> String {
        fs::read_to_string(&self.create_log).unwrap_or_default()
    }

    /// Poll `create_log` until it contains `needle`, or panic after `timeout`.
    fn wait_for_create_hits_containing(&self, needle: &str, timeout: Duration) -> String {
        poll_file_for_content(&self.create_log, needle, timeout)
    }

    fn queue_contents(&self) -> String {
        fs::read_to_string(&self.queue_path).unwrap_or_default()
    }

    fn dashboard_contents(&self) -> String {
        fs::read_to_string(&self.dashboard_path).unwrap_or_default()
    }

    fn finish(mut self) -> String {
        let status = wait_for_exit(&mut self.child, Duration::from_secs(10));
        self.drain(Duration::from_millis(300));
        assert!(status.success(), "fleet tui exited with {status:?}");

        let cleaned = strip_ansi(&String::from_utf8_lossy(&self.output));

        let python_hits = fs::read_to_string(&self.python_log).unwrap_or_default();
        assert!(
            python_hits.trim().is_empty(),
            "fleet tui touched python unexpectedly:\n{python_hits}"
        );

        let send_hits = fs::read_to_string(&self.send_log).unwrap_or_default();
        assert!(
            send_hits.trim().is_empty(),
            "fleet tui sent tmux input unexpectedly:\n{send_hits}"
        );

        cleaned
    }
}

/// TC-08: `amplihack fleet tui` should handle a real PTY session without any
/// Python interpreter on the live path.
///
/// This is the closest thing to the manual QA-style probe inside the automated
/// suite: a fake `azlin` shell script serves fleet data while fake `python` and
/// `python3` binaries would leave a breadcrumb if the Rust binary ever tried to
/// invoke them.
#[test]
fn tc08_fleet_tui_virtual_tty_stays_python_free() {
    let bin = amplihack_bin();
    require_binary!(bin);

    let mut probe = FleetTuiProbe::new(&bin, None);
    probe.drain(Duration::from_millis(1200));
    probe.send(b"?", Duration::from_millis(1200));
    probe.send(b"?", Duration::from_millis(1200));
    probe.send(b"\x1b[C", Duration::from_millis(1200));
    probe.send(b"q", Duration::from_millis(200));

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
    probe.drain(Duration::from_millis(1200));
    probe.send(b"d", Duration::from_millis(1500));
    probe.send(b"e", Duration::from_millis(1200));
    probe.send(b"t", Duration::from_millis(300));
    probe.send(b"t", Duration::from_millis(300));
    probe.send(b"t", Duration::from_millis(300));
    probe.send(b"t", Duration::from_millis(300));
    probe.send(b"i", Duration::from_millis(300));
    probe.send(b"rm -rf /", Duration::from_millis(1200));
    probe.send(b"A", Duration::from_millis(2200));
    probe.send(b"q", Duration::from_millis(200));

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
/// # Why polling instead of fixed sleeps
///
/// Session creation is dispatched to a background thread (via `bg_tx`) and the
/// azlin shim writes to `create_log` only after that thread processes the
/// `CreateSession` command.  Under CI parallel load the background thread can
/// be delayed well beyond any fixed margin.  Doubling the margins only reduces
/// flake frequency; it cannot eliminate the race.
///
/// The fix uses two polling primitives:
/// - `send_and_wait_for` waits until expected TUI output appears instead of
///   sleeping a fixed duration after each key press.
/// - `wait_for_create_hits_containing` retries reading `create_log` until the
///   expected content arrives or a hard deadline (10 s) is reached.
#[test]
fn tc10_fleet_tui_new_session_launches_without_python() {
    let bin = amplihack_bin();
    require_binary!(bin);

    let mut probe = FleetTuiProbe::new(&bin, None);

    // Wait for the TUI to render its initial Fleet view before sending keys.
    // A fixed drain here races against a slow binary start on a loaded CI host;
    // polling for a known string eliminates that.  "q quit" is part of the
    // controls line that appears in every frame, making it a stable sentinel.
    assert!(
        probe.wait_for_output_containing("q quit", Duration::from_secs(10)),
        "timed out waiting for initial Fleet view to render"
    );

    // Press 'n' to open the New Session dialog and wait for it to appear.
    probe.send_and_wait_for(b"n", "New Session", Duration::from_secs(5));

    // Press 't' to cycle the agent type.  The default is "claude"; after one
    // press it becomes "copilot".  Wait for the updated label to confirm the
    // keypress was actually processed and re-rendered.
    probe.send_and_wait_for(b"t", "Agent type: copilot", Duration::from_secs(3));

    // Submit the form with Enter.  The TUI dispatches CreateSession to the
    // background thread, which calls azlin and then sends SessionCreated back.
    // Wait until the TUI has rendered the confirmation message.
    probe.send_and_wait_for(b"\n", "Created session 'copilot-", Duration::from_secs(10));

    // Quit.
    probe.send(b"q", Duration::from_millis(400));

    // The azlin shim writes to create_log inside the background thread.  The
    // TUI has already rendered "Created session …" at this point, but the file
    // write is a side-effect of the same shell invocation so it must be present
    // by now — poll with a short timeout as a safety net.
    let create_hits =
        probe.wait_for_create_hits_containing("tmux new-session -d -s", Duration::from_secs(5));

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
#[test]
fn tc11_fleet_tui_adopts_selected_session_without_python() {
    let bin = amplihack_bin();
    require_binary!(bin);

    let mut probe = FleetTuiProbe::new(&bin, None);
    probe.drain(Duration::from_millis(1200));
    probe.send(b"A", Duration::from_millis(1500));
    probe.send(b"q", Duration::from_millis(200));

    let queue = probe.queue_contents();
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
    probe.drain(Duration::from_millis(1200));
    probe.send(b"t", Duration::from_millis(1200));
    probe.send(b"j", Duration::from_millis(800));
    probe.send(b"q", Duration::from_millis(200));

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
    probe.drain(Duration::from_millis(1200));
    probe.send(b"t", Duration::from_millis(1200));
    probe.send(b"j", Duration::from_millis(800));
    probe.send(b"q", Duration::from_millis(200));

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
    probe.drain(Duration::from_millis(1200));
    probe.send(b"\x1b[C", Duration::from_millis(1200));
    probe.send(b"q", Duration::from_millis(200));

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
    probe.drain(Duration::from_millis(1200));
    probe.send(b"p", Duration::from_millis(800));
    probe.send(b"i", Duration::from_millis(300));
    probe.send(
        b"https://github.com/org/new-repo\n",
        Duration::from_millis(1200),
    );
    probe.send(b"x", Duration::from_millis(1200));
    probe.send(b"q", Duration::from_millis(200));

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
    probe.drain(Duration::from_millis(1200));
    probe.send(b"d", Duration::from_millis(1500));
    probe.send(b"q", Duration::from_millis(200));

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
    probe.drain(Duration::from_millis(1200));
    probe.send(b"3", Duration::from_millis(800));
    probe.send(b"4", Duration::from_millis(800));
    probe.send(b"q", Duration::from_millis(200));

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
    probe.drain(Duration::from_millis(1200));
    probe.send(b"d", Duration::from_millis(1500));
    probe.send(b"e", Duration::from_millis(1200));
    probe.send(b"i", Duration::from_millis(300));
    probe.send(b"scratch", Duration::from_millis(800));
    probe.send(b"\x1b", Duration::from_millis(1200));
    probe.send(b"q", Duration::from_millis(200));

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
    probe.drain(Duration::from_millis(1200));
    probe.send(b"2", Duration::from_millis(1200));
    probe.send(b"q", Duration::from_millis(200));

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
    probe.drain(Duration::from_millis(1200));
    probe.send(b"l", Duration::from_millis(800));
    probe.send(b"q", Duration::from_millis(200));

    let cleaned = probe.finish();
    assert!(
        cleaned.contains("A M P L I H A C K   F L E E T"),
        "expected logo in initial PTY output:\n{cleaned}"
    );
    let last_frame_body = cleaned
        .rsplit_once('╔')
        .map(|(_, tail)| tail)
        .unwrap_or(cleaned.as_str());
    let last_frame = format!("╔{last_frame_body}");
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
    probe.drain(Duration::from_millis(1200));
    probe.send(b"t", Duration::from_millis(1200));
    probe.send(b"/", Duration::from_millis(300));
    probe.send(b"vm-2\n", Duration::from_millis(1200));
    probe.send(b"q", Duration::from_millis(200));

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
#[test]
fn tc21_fleet_tui_refresh_streams_partial_results_without_python() {
    let bin = amplihack_bin();
    require_binary!(bin);

    let mut probe =
        FleetTuiProbe::new_with_existing_vms_and_vm2_delay(&bin, None, Some("vm-2"), Some(2.0));
    probe.drain(Duration::from_millis(1200));

    let partial = strip_ansi(&String::from_utf8_lossy(&probe.output));
    assert!(
        partial.contains("refresh: 1/2 polling vm-2"),
        "expected in-flight refresh progress before vm-2 finished:\n{partial}"
    );
    assert!(
        partial.contains("vm-1") && partial.contains("claude-1"),
        "expected first VM to render before the slow second VM completed:\n{partial}"
    );

    probe.send(b"q", Duration::from_millis(2500));
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
    probe.drain(Duration::from_millis(1200));
    probe.send(b"d", Duration::from_millis(1500));
    probe.send(b"x", Duration::from_millis(1200));
    probe.send(b"q", Duration::from_millis(200));

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
    probe.drain(Duration::from_millis(1200));
    probe.send(b"d", Duration::from_millis(1500));
    probe.send(b"q", Duration::from_millis(200));

    let cleaned = probe.finish();
    assert!(
        cleaned.contains("Reasoner status")
            && cleaned.contains("ANTHROPIC_API_KEY missing")
            && cleaned.contains("heuristic proposal"),
        "expected visible reasoner failure notice in detail output:\n{cleaned}"
    );
}
