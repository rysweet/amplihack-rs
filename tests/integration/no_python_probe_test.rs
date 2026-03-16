/// Integration tests: AC9 no-Python probe — extended coverage (Issue #77).
///
/// These tests validate that the amplihack binary handles code-graph subcommands
/// (`index-code`, `index-scip`, `query-code`) correctly in a Python-free
/// environment.
///
/// # Status
///
/// All tests TC-01 through TC-07 pass.  The probe script
/// (`scripts/probe-no-python.sh`) has been extended to cover TC-04 through
/// TC-07.  AC9 is fully satisfied.
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

    // Create a temp file path for the Kuzu DB.  We pass the path to the
    // binary so it creates a fresh database — we do NOT pre-populate it.
    let temp_dir = tempfile::TempDir::new().expect("failed to create tempdir");
    let db_path = temp_dir.path().join("probe_tc06.kuzu");

    let output = cmd_without_python(&bin)
        .args([
            "query-code",
            "--db",
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

/// Validate that the probe shell script exits 0 and covers TC-04 through TC-07.
///
/// Checks both:
///   1. The script exits 0 (all smoke tests pass)
///   2. The output proves TC-04 through TC-07 were exercised
///
/// Skipped when the script is not executable (e.g., fresh clone without
/// execute bit set).
#[test]
fn probe_script_exits_zero_and_covers_tc04_through_tc07() {
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

    // Gate 2: script output proves it ran TC-04 through TC-07.
    let has_tc04 = combined.contains("index-code");
    let has_tc05 = combined.contains("query-code");
    let has_tc06 = combined.contains("mktemp") || combined.contains("stats");
    let has_tc07 = combined.contains("index-scip");
    assert!(
        has_tc04 && has_tc05 && has_tc06 && has_tc07,
        "FAIL: probe script ran successfully but did not exercise TC-04 through TC-07.\n\
         Missing: tc04(index-code)={has_tc04}, tc05(query-code)={has_tc05}, \
         tc06(stats/mktemp)={has_tc06}, tc07(index-scip)={has_tc07}\n\
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
    output: Vec<u8>,
}

impl FleetTuiProbe {
    fn new(bin: &Path, reasoner_json: Option<&str>) -> Self {
        Self::new_with_existing_vms(bin, reasoner_json, None)
    }

    fn new_with_existing_vms(
        bin: &Path,
        reasoner_json: Option<&str>,
        existing_vms: Option<&str>,
    ) -> Self {
        let temp_dir = tempfile::TempDir::new().expect("failed to create tempdir");
        let fake_bin = temp_dir.path().join("bin");
        fs::create_dir_all(&fake_bin).expect("failed to create fake bin dir");

        let python_log = temp_dir.path().join("python.log");
        let send_log = temp_dir.path().join("send.log");
        let create_log = temp_dir.path().join("create.log");
        let queue_path = temp_dir.path().join(".amplihack/fleet/task_queue.json");
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
      printf '%s\n' 'Proceed with deploy? [y/n]'
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

        if let Some(json) = reasoner_json {
            let reasoner = fake_bin.join("claude");
            write_executable(&reasoner, &format!("#!/bin/sh\ncat <<'EOF'\n{json}\nEOF\n"));
            command
                .env("AMPLIHACK_FLEET_REASONER_BINARY_PATH", &reasoner)
                .env("AMPLIHACK_CLAUDE_BINARY_PATH", reasoner);
        }

        let child = command.spawn().expect("failed to spawn fleet tui");

        Self {
            _temp_dir: temp_dir,
            master,
            child,
            python_log,
            send_log,
            create_log,
            queue_path,
            output: Vec::new(),
        }
    }

    fn drain(&mut self, duration: Duration) {
        read_until_quiet(&mut self.master, &mut self.output, duration);
    }

    fn send(&mut self, bytes: &[u8], wait: Duration) {
        self.master
            .write_all(bytes)
            .unwrap_or_else(|error| panic!("failed to send PTY input {bytes:?}: {error}"));
        self.drain(wait);
    }

    fn create_hits(&self) -> String {
        fs::read_to_string(&self.create_log).unwrap_or_default()
    }

    fn queue_contents(&self) -> String {
        fs::read_to_string(&self.queue_path).unwrap_or_default()
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
    probe.send(b"rm -rf /\n", Duration::from_millis(1200));
    probe.send(b"A", Duration::from_millis(1200));
    probe.send(b"q", Duration::from_millis(200));

    let cleaned = probe.finish();
    assert!(
        cleaned.contains("Action Editor") && cleaned.contains("Action: send_input"),
        "expected proposal/editor output in PTY flow:\n{cleaned}"
    );
    assert!(
        cleaned.contains("dangerous-input policy"),
        "expected dangerous-input block in PTY output:\n{cleaned}"
    );
}

/// TC-10: the native TUI new-session flow should create a session over azlin
/// without touching Python or sending tmux input to an existing session.
#[test]
fn tc10_fleet_tui_new_session_launches_without_python() {
    let bin = amplihack_bin();
    require_binary!(bin);

    let mut probe = FleetTuiProbe::new(&bin, None);
    probe.drain(Duration::from_millis(1200));
    probe.send(b"n", Duration::from_millis(800));
    probe.send(b"t", Duration::from_millis(300));
    probe.send(b"\n", Duration::from_millis(1200));
    probe.send(b"q", Duration::from_millis(200));

    let create_hits = probe.create_hits();
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
